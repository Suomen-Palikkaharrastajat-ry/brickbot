//! Recursive-descent parser for the build language.
//!
//! Context-free by design: the parser never looks at the model. That keeps it
//! a pure function of a string, so the whole grammar can be table-tested, and
//! it puts reference resolution — which genuinely does need the model — in one
//! place, [`super::lower`].
//!
//! The error policy matters as much as the grammar. A sentence that does not
//! open with a verb or a question word never reaches the parser at all
//! (see [`Sentence::looks_like_command`]), so ordinary conversation in the
//! thread stays silent. A sentence that *does* open with a verb but fails to
//! parse is reported, because the builder clearly meant something.

#![allow(clippy::doc_markdown, clippy::too_many_lines)]

use super::ast::{
    Arrangement, Command, ExtentAxis, GridLayer, GridSpec, LegendEntry, Location, Meta, Modifier,
    ObjectExpr, PartSpec, Place, Query, Ref, Statement, Structure,
};
use super::block::{self, CodeBlock};
use super::lexer::{HelpTopic, Keyword, Sentence, Token, Verb};
use crate::model::PartKind;
use crate::units::{Direction, GridView, IVec3, Unit};

mod command;
mod phrases;
mod query;

use command::{parse_command, parse_name};
pub use phrases::parse_part_spec;
use query::{grid_spec, parse_proposal, parse_query, parse_view_query};

/// What the parser was looking for when it gave up.
///
/// A closed set rather than free text, so the phrasing lives in the locale
/// bundles with the rest of the user-facing strings and `report.rs` stays the
/// only place that turns one into a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expectation {
    RevisionNumber,
    SomethingToFind,
    SomethingToShow,
    Name,
    SomethingToBuild,
    PartToAdd,
    ColourToPaintWith,
    SomethingToReplace,
    ReplacementPartOrColour,
    SomethingToSelect,
    GroupName,
    RightAngle,
    ViewName,
}

impl Expectation {
    /// Localization key of the phrase describing what was expected.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::RevisionNumber => "command.build.expected.revision_number",
            Self::SomethingToFind => "command.build.expected.something_to_find",
            Self::SomethingToShow => "command.build.expected.something_to_show",
            Self::Name => "command.build.expected.name",
            Self::SomethingToBuild => "command.build.expected.something_to_build",
            Self::PartToAdd => "command.build.expected.part_to_add",
            Self::ColourToPaintWith => "command.build.expected.colour_to_paint_with",
            Self::SomethingToReplace => "command.build.expected.something_to_replace",
            Self::ReplacementPartOrColour => "command.build.expected.replacement_part_or_colour",
            Self::SomethingToSelect => "command.build.expected.something_to_select",
            Self::GroupName => "command.build.expected.group_name",
            Self::RightAngle => "command.build.expected.right_angle",
            Self::ViewName => "command.build.expected.view_name",
        }
    }
}

/// Why a sentence that looked like a command could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// What the parser was looking for.
    pub expected: Expectation,
    /// The word it stopped on, if any.
    pub found: Option<String>,
    /// The sentence as typed.
    pub sentence: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.found {
            Some(found) => write!(f, "expected {:?} but found '{found}'", self.expected),
            None => write!(f, "expected {:?}", self.expected),
        }
    }
}

/// What a single sentence turned into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentenceOutcome {
    /// A statement the interpreter can act on.
    Statement(Statement),
    /// Ordinary chat. The bot says nothing.
    Ignored,
    Error(ParseError),
}

#[derive(Clone, Copy)]
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// One past the last token this parser may read.
    ///
    /// Temporarily narrowed when a trailing token has already been claimed —
    /// `Paint selection dark tan.` puts the colour at the very end, so the
    /// object must be parsed from everything *before* it or the part
    /// description would swallow the colour.
    end: usize,
    sentence: &'a str,
    /// The drawing that followed this sentence, if there was one. A grid is an
    /// argument to a sentence rather than part of its token stream, so the
    /// header parses with the ordinary machinery — locations included.
    block: Option<&'a CodeBlock>,
    /// An angle that is not a whole number of quarter turns, so the sentence
    /// can be rejected rather than silently rounded.
    bad_angle: Option<i64>,
}

impl<'a> Parser<'a> {
    fn new(sentence: &'a Sentence) -> Self {
        Self {
            tokens: &sentence.tokens,
            pos: 0,
            end: sentence.tokens.len(),
            sentence: &sentence.text,
            block: sentence.block.as_ref(),
            bad_angle: None,
        }
    }

    const fn has_block(&self) -> bool {
        self.block.is_some()
    }

    /// Reads a view name, which may arrive as either kind of token: `top` and
    /// `bottom` are bare words, while `north` and `above` lex as directions.
    fn grid_view(&mut self) -> Option<GridView> {
        let view = match self.peek() {
            Some(Token::Direction(direction)) => GridView::from_direction(*direction),
            Some(Token::Word(word)) => GridView::parse(word),
            _ => None,
        }?;
        self.pos += 1;
        Some(view)
    }

    /// True when what follows could name a view, so a drawing header can be
    /// told apart from `Build a wall …` without consuming anything.
    ///
    /// A leading `from` counts on its own: nothing else in the language is
    /// built *from* something, so `Build from sideways:` with a drawing after
    /// it is a misspelled view rather than an unknown structure, and saying so
    /// is more use than "I expected something to build".
    fn looks_like_grid_header(&self) -> bool {
        let mut probe = Parser {
            pos: self.pos,
            block: None,
            ..*self
        };
        probe.skip_filler();
        if probe.eat_keyword(Keyword::From) {
            return true;
        }
        probe.skip_filler();
        probe.grid_view().is_some()
    }

    /// Reads `layer 2`, `block 2` or `slice 2`, counted from one.
    fn layer_index(&mut self) -> Option<u32> {
        let start = self.pos;
        self.skip_filler();
        let named = self.eat_word("layer") || self.eat_word("block") || self.eat_word("slice");
        if !named {
            self.pos = start;
            return None;
        }
        match self.number() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(n) if n >= 1 => Some(n as u32),
            _ => {
                self.pos = start;
                None
            }
        }
    }

    fn peek(&self) -> Option<&Token> {
        if self.pos >= self.end {
            return None;
        }
        self.tokens.get(self.pos)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        if self.pos + offset >= self.end {
            return None;
        }
        self.tokens.get(self.pos + offset)
    }

    fn next(&mut self) -> Option<&Token> {
        if self.pos >= self.end {
            return None;
        }
        let token = self.tokens.get(self.pos);
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    const fn at_end(&self) -> bool {
        self.pos >= self.end
    }

    /// The index of the last colour token at or after `from`, if any.
    fn last_color_index(&self, from: usize) -> Option<usize> {
        (from..self.end)
            .rev()
            .find(|i| matches!(self.tokens.get(*i), Some(Token::Color(_))))
    }

    /// Consumes the token if it is this keyword.
    fn eat_keyword(&mut self, keyword: Keyword) -> bool {
        if self.peek() == Some(&Token::Keyword(keyword)) {
            self.pos += 1;
            return true;
        }
        false
    }

    /// Consumes any of these keywords, returning which one matched.
    fn eat_any_keyword(&mut self, keywords: &[Keyword]) -> Option<Keyword> {
        if let Some(Token::Keyword(k)) = self.peek() {
            if keywords.contains(k) {
                let found = *k;
                self.pos += 1;
                return Some(found);
            }
        }
        None
    }

    /// Consumes a specific bare word.
    fn eat_word(&mut self, word: &str) -> bool {
        if self.peek().and_then(Token::as_word) == Some(word) {
            self.pos += 1;
            return true;
        }
        false
    }

    /// Skips articles and other filler that carry no meaning in position.
    fn skip_filler(&mut self) {
        while let Some(token) = self.peek() {
            match token {
                Token::Word(w) if w == "the" => self.pos += 1,
                // A leading "a"/"an" lexes as the number 1; it is filler
                // unless a real count follows.
                Token::Number(1)
                    if !matches!(self.peek_at(1), Some(Token::Keyword(Keyword::Times))) =>
                {
                    self.pos += 1;
                }
                _ => break,
            }
        }
    }

    fn error(&self, expected: Expectation) -> ParseError {
        ParseError {
            expected,
            found: self.peek().and_then(Token::quotable),
            sentence: self.sentence.to_string(),
        }
    }

    /// Reads a count, accepting both digits and number words.
    fn number(&mut self) -> Option<i64> {
        if let Some(Token::Number(n)) = self.peek() {
            let value = *n;
            self.pos += 1;
            return Some(value);
        }
        None
    }
}

/// The topic a `Help …` sentence names, if it names one.
///
/// Takes the first word after `help` that resolves to a topic, so `Help me with
/// drawings.` and `Help the select.` both land where they were meant to. The
/// sentence is read as typed rather than from the token stream, which has
/// already collapsed `draw` into [`Verb::Render`] and `colour` into
/// [`Verb::Paint`] — the two words most likely to be asked about by name.
fn help_topic(sentence: &str) -> Option<HelpTopic> {
    sentence
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .skip(1)
        .find_map(|word| HelpTopic::parse(&word.to_lowercase()))
}

#[must_use]
pub fn parse_sentence(sentence: &Sentence) -> SentenceOutcome {
    if sentence.is_comment {
        return SentenceOutcome::Statement(Statement::Comment);
    }
    if !sentence.looks_like_command() {
        return SentenceOutcome::Ignored;
    }

    let mut parser = Parser::new(sentence);

    // Questions come first, since they open with a question word rather than
    // a verb.
    if let Some(query) = parse_query(&mut parser) {
        return SentenceOutcome::Statement(Statement::Query(query));
    }

    let Some(Token::Verb(verb)) = parser.peek().copied_verb() else {
        // A part named with no verb — "2x2 brick" — is not a command but is
        // plainly about building, so it asks where the part could go.
        if let Some(statement) = parse_proposal(&mut Parser::new(sentence)) {
            return SentenceOutcome::Statement(statement);
        }
        // Nothing but a question word got this sentence past the gate, and it
        // is not a question we understand — so it is ordinary curiosity
        // ("what do you think?"), not a mistyped command. Reporting it would
        // break the silence policy the whole feature rests on. A sentence that
        // opened with a *verb* still falls through to an error below, because
        // there the builder clearly meant something.
        return SentenceOutcome::Ignored;
    };
    parser.pos += 1;

    match verb {
        Verb::Undo => {
            // "Undo alice's last edit."
            parser.skip_filler();
            if let Some(Token::Word(name)) = parser.peek() {
                let name = name.trim_end_matches("'s").to_string();
                return SentenceOutcome::Statement(Statement::Meta(Meta::UndoBy(name)));
            }
            SentenceOutcome::Statement(Statement::Meta(Meta::Undo))
        }
        Verb::Redo => SentenceOutcome::Statement(Statement::Meta(Meta::Redo)),
        Verb::Revert => {
            parser.skip_filler();
            parser.eat_keyword(Keyword::To);
            parser.eat_keyword(Keyword::Revision);
            parser.skip_filler();
            match parser.number() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(n) if n >= 0 => {
                    SentenceOutcome::Statement(Statement::Meta(Meta::RevertTo(n as u32)))
                }
                _ => SentenceOutcome::Error(parser.error(Expectation::RevisionNumber)),
            }
        }
        Verb::Export => SentenceOutcome::Statement(Statement::Meta(Meta::Export)),
        Verb::Render => SentenceOutcome::Statement(Statement::Meta(Meta::Render)),
        Verb::Help => {
            // "Help select." names a topic; a bare "Help." does not. Anything
            // else after "help" that isn't a topic (a typo, "help me") is
            // quietly ignored rather than reported — asking for help wrong is
            // still asking for help.
            //
            // Read from the sentence as typed rather than from the tokens:
            // tokenizing has already thrown away which word produced a verb,
            // and "Help draw." must reach drawings while "Help render." reaches
            // render, even though both lex to `Verb::Render`.
            let topic = help_topic(parser.sentence);
            SentenceOutcome::Statement(Statement::Meta(Meta::Help(topic)))
        }
        Verb::Finish => SentenceOutcome::Statement(Statement::Meta(Meta::Finish)),
        Verb::Count => {
            let spec = parser.part_spec();
            SentenceOutcome::Statement(Statement::Query(Query::Count(Box::new(spec))))
        }
        Verb::Find => {
            if parser.eat_word("collisions") {
                return SentenceOutcome::Statement(Statement::Query(Query::FindCollisions));
            }
            SentenceOutcome::Error(parser.error(Expectation::SomethingToFind))
        }
        Verb::Show => {
            if parser.eat_word("unsupported") {
                return SentenceOutcome::Statement(Statement::Query(Query::ShowUnsupported));
            }
            if parser.eat_word("collisions") {
                return SentenceOutcome::Statement(Statement::Query(Query::FindCollisions));
            }
            if parser.eat_word("parts") || parser.eat_word("inventory") {
                return SentenceOutcome::Statement(Statement::Query(Query::ListParts));
            }
            if let Some(query) = parse_view_query(&mut parser) {
                return SentenceOutcome::Statement(Statement::Query(query));
            }
            SentenceOutcome::Error(parser.error(Expectation::SomethingToShow))
        }
        Verb::Name => parse_name(&mut parser),
        _ => parse_command(&mut parser, verb),
    }
}

/// Helper so the borrow of `peek` ends before `pos` is advanced.
trait CopiedVerb {
    fn copied_verb(self) -> Option<Token>;
}

impl CopiedVerb for Option<&Token> {
    fn copied_verb(self) -> Option<Token> {
        match self {
            Some(Token::Verb(v)) => Some(Token::Verb(*v)),
            _ => None,
        }
    }
}

/// Parses a whole chat message.
///
/// Returns one outcome per sentence, in order.
#[must_use]
pub fn parse_message(text: &str) -> Vec<SentenceOutcome> {
    super::lexer::tokenize(text)
        .iter()
        .map(parse_sentence)
        .collect()
}

/// Stands in when a grid header somehow has no block, which the guard on the
/// branch already rules out. A drawing with nothing in it is reported by
/// lowering as `NoLegend`, the same as an empty fence would be.
static EMPTY_BLOCK: CodeBlock = CodeBlock { lines: Vec::new() };

#[cfg(test)]
mod tests;
