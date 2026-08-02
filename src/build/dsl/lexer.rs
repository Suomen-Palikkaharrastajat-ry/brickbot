//! Tokenizer for the build language.
//!
//! Turns a chat message into sentences of classified tokens. This stage knows
//! nothing about grammar or about the model — it only recognises words.
//!
//! Two details do real work here. Colours are multi-word (`dark bluish grey`),
//! so the lexer matches them greedily against the palette rather than leaving
//! the parser to guess how many words to take. And Discord noise — mentions,
//! custom emoji, code fences, URLs — is stripped up front, because those
//! appear constantly in a chat channel and would otherwise turn ordinary
//! conversation into parse errors.

#![allow(clippy::doc_markdown)]

use super::block::{self, CodeBlock, Segment};
use crate::build::model::PartKind;
use crate::build::units::{Direction, Unit};
use crate::ldraw::colors;

/// A verb the build language recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    Add,
    Move,
    Rotate,
    Remove,
    Replace,
    Paint,
    Build,
    Duplicate,
    Mirror,
    Attach,
    Select,
    Group,
    Name,
    Undo,
    Redo,
    Revert,
    Queries,
    Count,
    Find,
    Show,
    Export,
    Render,
    Help,
    Finish,
    Repeat,
    Fill,
    Raise,
    Lower,
}

impl Verb {
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "add" | "place" | "put" | "insert" => Self::Add,
            "move" | "shift" | "slide" => Self::Move,
            "raise" => Self::Raise,
            "lower" | "drop" => Self::Lower,
            "rotate" | "turn" | "face" | "spin" => Self::Rotate,
            "remove" | "delete" | "erase" => Self::Remove,
            "replace" | "swap" => Self::Replace,
            "paint" | "colour" | "color" | "recolour" | "recolor" => Self::Paint,
            "build" | "make" | "construct" => Self::Build,
            "duplicate" | "copy" | "clone" => Self::Duplicate,
            "mirror" | "reflect" => Self::Mirror,
            "attach" | "connect" | "snap" | "rest" | "stack" => Self::Attach,
            "select" | "highlight" => Self::Select,
            "group" | "ungroup" => Self::Group,
            "name" | "call" | "rename" => Self::Name,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            "revert" | "rewind" => Self::Revert,
            "query" | "queries" | "questions" => Self::Queries,
            "count" => Self::Count,
            "find" => Self::Find,
            "show" | "list" => Self::Show,
            "export" | "download" | "save" => Self::Export,
            "render" | "draw" => Self::Render,
            "help" | "commands" => Self::Help,
            "finish" | "done" | "end" => Self::Finish,
            "repeat" => Self::Repeat,
            "fill" => Self::Fill,
            _ => return None,
        })
    }

    /// Localization key for `Help <verb>.`'s per-topic syntax and examples.
    ///
    /// Synonyms resolve to their canonical verb's topic. `Queries` exists
    /// to let `Help queries.` document read-only operations that aren't
    /// tied to a single verb.
    #[must_use]
    pub const fn help_key(self) -> Option<&'static str> {
        Some(match self {
            Self::Add | Self::Attach | Self::Fill => "command.build.help_topic.add",
            Self::Move => "command.build.help_topic.move",
            Self::Raise => "command.build.help_topic.raise",
            Self::Lower => "command.build.help_topic.lower",
            Self::Rotate => "command.build.help_topic.rotate",
            Self::Remove => "command.build.help_topic.remove",
            Self::Replace => "command.build.help_topic.replace",
            Self::Paint => "command.build.help_topic.paint",
            Self::Build => "command.build.help_topic.build",
            Self::Duplicate | Self::Repeat => "command.build.help_topic.duplicate",
            Self::Mirror => "command.build.help_topic.mirror",
            Self::Select => "command.build.help_topic.select",
            Self::Group => "command.build.help_topic.group",
            Self::Name => "command.build.help_topic.name",
            Self::Undo => "command.build.help_topic.undo",
            Self::Redo => "command.build.help_topic.redo",
            Self::Revert => "command.build.help_topic.revert",
            Self::Count => "command.build.help_topic.count",
            Self::Find => "command.build.help_topic.find",
            Self::Show => "command.build.help_topic.show",
            Self::Export => "command.build.help_topic.export",
            Self::Render => "command.build.help_topic.render",
            Self::Help => "command.build.help_topic.help",
            Self::Finish => "command.build.help_topic.finish",
            Self::Queries => "command.build.help_topic.queries",
        })
    }
}

/// A topic `Help <topic>.` can name.
///
/// Not every topic is a verb. Drawings, locations, parts and the session
/// buttons are things a builder asks about by name, and drawings are the whole
/// reason this type exists: `draw` lexes as a synonym of [`Verb::Render`], so a
/// verb-only topic list answers `Help draw.` with "render the model again" —
/// the opposite of what was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpTopic {
    Verb(Verb),
    Drawings,
    Locations,
    Parts,
    Buttons,
}

impl HelpTopic {
    /// Every topic that is not a verb, in the order `Help.` lists them.
    pub const NAMED: [Self; 4] = [Self::Drawings, Self::Locations, Self::Parts, Self::Buttons];

    /// Resolves the word after `help` to a topic.
    ///
    /// Named topics are tried **before** verbs, so a word that is both — `draw`
    /// (also `render`) and `colour` (also `paint`) — answers the question that
    /// was actually asked. The verbs themselves are still reachable by their
    /// canonical names: `Help render.` and `Help paint.` are unaffected.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "drawing" | "drawings" | "draw" | "grid" | "grids" => Self::Drawings,
            "location" | "locations" | "landmark" | "landmarks" => Self::Locations,
            "part" | "parts" | "colour" | "colours" | "color" | "colors" | "size" | "sizes" => {
                Self::Parts
            }
            "button" | "buttons" | "draft" | "drafts" | "private" => Self::Buttons,
            _ => Self::Verb(Verb::parse(word)?),
        })
    }

    /// Localization key for this topic's syntax and examples.
    #[must_use]
    pub const fn help_key(self) -> Option<&'static str> {
        match self {
            Self::Verb(verb) => verb.help_key(),
            Self::Drawings => Some("command.build.help_topic.drawings"),
            Self::Locations => Some("command.build.help_topic.locations"),
            Self::Parts => Some("command.build.help_topic.parts"),
            Self::Buttons => Some("command.build.help_topic.buttons"),
        }
    }
}

/// Prepositions and other function words the grammar branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    At,
    On,
    Under,
    Of,
    To,
    By,
    With,
    In,
    From,
    Across,
    Into,
    And,
    All,
    Everything,
    That,
    It,
    Them,
    This,
    These,
    Those,
    Last,
    Previous,
    Origin,
    Selection,
    Times,
    Degrees,
    Long,
    High,
    Wide,
    Deep,
    Tall,
    Steps,
    Revision,
    Upside,
    Point,
    Current,
    Between,
}

impl Keyword {
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "at" => Self::At,
            "on" | "onto" | "atop" => Self::On,
            "under" | "beneath" | "underneath" => Self::Under,
            "of" => Self::Of,
            "to" => Self::To,
            "by" => Self::By,
            "with" | "using" => Self::With,
            "in" | "inside" => Self::In,
            "from" => Self::From,
            "across" | "about" => Self::Across,
            "into" => Self::Into,
            "and" => Self::And,
            "all" | "every" => Self::All,
            "everything" => Self::Everything,
            "that" => Self::That,
            "it" => Self::It,
            "them" => Self::Them,
            "this" => Self::This,
            "these" => Self::These,
            "those" => Self::Those,
            "last" | "latest" => Self::Last,
            "previous" => Self::Previous,
            "origin" | "centre" | "center" => Self::Origin,
            "selection" => Self::Selection,
            "times" => Self::Times,
            "degrees" | "degree" => Self::Degrees,
            "long" => Self::Long,
            "high" => Self::High,
            "wide" => Self::Wide,
            "deep" => Self::Deep,
            "tall" | "taller" => Self::Tall,
            "step" | "steps" => Self::Steps,
            "revision" | "rev" => Self::Revision,
            "upside" => Self::Upside,
            "point" => Self::Point,
            "current" => Self::Current,
            "between" => Self::Between,
            _ => return None,
        })
    }
}

/// One classified word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Verb(Verb),
    Keyword(Keyword),
    Direction(Direction),
    Unit(Unit),
    Kind(PartKind),
    Color(u16),
    /// A part size such as `2x4`.
    Dim(u8, u8),
    Number(i64),
    /// A word we do not recognise: a name, a part number, or plain chat.
    Word(String),
    /// A double-quoted string, kept verbatim.
    Quoted(String),
}

impl Token {
    #[must_use]
    pub fn as_word(&self) -> Option<&str> {
        match self {
            Self::Word(w) | Self::Quoted(w) => Some(w),
            _ => None,
        }
    }

    /// The token as it can be quoted back at the builder, if it can be.
    ///
    /// Only tokens that carry their own text, or that map back to one obvious
    /// word, are quotable. Everything else answers `None` so the caller drops
    /// the "I got as far as …" clause — a parse error must never show a Rust
    /// type name to somebody who was building a castle.
    #[must_use]
    pub fn quotable(&self) -> Option<String> {
        match self {
            Self::Word(w) | Self::Quoted(w) => Some(w.clone()),
            Self::Number(n) => Some(n.to_string()),
            Self::Dim(w, d) => Some(format!("{w}x{d}")),
            Self::Kind(kind) => Some(kind.as_str().to_string()),
            Self::Direction(direction) => Some(direction.as_str().to_string()),
            Self::Unit(unit) => Some(unit.as_str().to_string()),
            Self::Color(code) => crate::ldraw::colors::name_of(*code).map(str::to_string),
            Self::Verb(_) | Self::Keyword(_) => None,
        }
    }
}

/// One sentence's worth of tokens, plus the text it came from for error
/// messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub tokens: Vec<Token>,
    pub text: String,
    /// True when the sentence began with a comment marker such as `Note:`.
    pub is_comment: bool,
    /// The fenced block that followed this sentence, if any.
    ///
    /// A drawing is not a sentence — it is an argument to one, the way
    /// `10 studs long` is. Keeping it here rather than in the token stream is
    /// what lets the grid grammar reuse the ordinary sentence machinery for
    /// its header, locations included.
    pub block: Option<CodeBlock>,
}

impl Sentence {
    /// The sentence as it must be re-sent to mean the same thing again.
    ///
    /// A sentence that paused on an unknown part is parked verbatim and
    /// replayed once the builder has picked one, so a drawing has to survive
    /// the round trip — without its fence, `Build from top:` means nothing.
    /// The fence is re-emitted canonically, which re-lexes to exactly this
    /// sentence again.
    #[must_use]
    pub fn replay_text(&self) -> String {
        self.block.as_ref().map_or_else(
            || self.text.clone(),
            |block| format!("{}\n```\n{}\n```", self.text, block.lines.join("\n")),
        )
    }

    /// True when the sentence opens with something the grammar could act on.
    ///
    /// This is the gate that keeps ordinary conversation out of the parser: a
    /// message that does not start with a verb, a question word or the name of
    /// a part is never reported as a parse error, it is simply ignored.
    #[must_use]
    pub fn looks_like_command(&self) -> bool {
        if self.is_comment {
            return true;
        }
        match self.tokens.first() {
            Some(Token::Verb(_)) => true,
            Some(Token::Word(w)) => {
                matches!(w.as_str(), "how" | "what" | "where" | "who" | "which")
            }
            // Naming a part with no verb — "2x2 brick" — asks where it could
            // go. A size or a kind has to be in there somewhere: a bare count
            // or colour is how people talk ("2 beers", "red"), and swallowing
            // those is what keeps a build thread liveable.
            Some(Token::Dim(..) | Token::Kind(_) | Token::Number(_) | Token::Color(_)) => {
                self.names_a_part()
            }
            _ => false,
        }
    }

    /// True when some token in the sentence pins down an actual element.
    fn names_a_part(&self) -> bool {
        self.tokens
            .iter()
            .any(|t| matches!(t, Token::Dim(..) | Token::Kind(_)))
    }
}

fn number_word(word: &str) -> Option<i64> {
    Some(match word {
        "zero" => 0,
        "one" | "a" | "an" => 1,
        "two" | "twice" => 2,
        "three" | "thrice" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "sixteen" => 16,
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "hundred" => 100,
        _ => return None,
    })
}

/// Strips the bits of a Discord message that are never part of a command.
#[must_use]
pub fn strip_discord_noise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_code = false;

    while let Some(c) = chars.next() {
        match c {
            '`' => {
                // Toggle on any run of backticks, so both inline code and
                // fenced blocks are dropped.
                while chars.peek() == Some(&'`') {
                    chars.next();
                }
                in_code = !in_code;
            }
            _ if in_code => {}
            '<' => {
                // Mentions, channel links and custom emoji: <@123>, <#456>,
                // <:name:789>. Drop up to the closing '>'.
                let mut consumed = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '>' {
                        closed = true;
                        break;
                    }
                    consumed.push(inner);
                    if consumed.len() > 64 {
                        break;
                    }
                }
                if !closed {
                    out.push('<');
                    out.push_str(&consumed);
                }
            }
            _ => out.push(c),
        }
    }

    // Drop anything that looks like a URL, and anything that is a Discord
    // block-quote, line by line. Filtering the whole string at once would
    // rejoin it with spaces and lose every newline — which are sentence
    // terminators, so a message typed as several lines would collapse into
    // one and everything after the first line would be silently swallowed by
    // the modifier parser.
    //
    // A quoted line is someone else's words, not the author's — most often
    // the bot's own instructions, quoted back while asking "did this work?".
    // Without this, a `> Add a red brick.` line is parsed exactly like a real
    // command and quietly repeats whatever was quoted.
    let mut in_block_quote = false;
    out.split('\n')
        .map(|line| {
            let trimmed_start = line.trim_start();
            if in_block_quote || is_quote_line(trimmed_start) {
                // `>>>` turns everything from here to the end of the message
                // into a quote, unlike a single `>` line.
                if trimmed_start.starts_with(">>>") {
                    in_block_quote = true;
                }
                return String::new();
            }
            line.split_whitespace()
                .filter(|w| !w.starts_with("http://") && !w.starts_with("https://"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a line is a Discord block-quote (`> …` or `>>> …`), rather than a
/// stray `>` inside ordinary text.
fn is_quote_line(trimmed_start: &str) -> bool {
    trimmed_start.strip_prefix('>').is_some_and(|rest| {
        rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('>')
    })
}

/// Splits text into sentences on terminators and newlines.
#[must_use]
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '.' | '!' | '?' | ';' | '\n' => {
                if !current.trim().is_empty() {
                    // Keep '?' so queries stay recognisable.
                    if c == '?' {
                        current.push('?');
                    }
                    sentences.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }
    sentences
}

/// A word as it came out of the raw text, before classification.
struct RawWord {
    text: String,
    /// True when it came from a double-quoted span, so it must be taken
    /// verbatim as a name rather than classified.
    quoted: bool,
}

/// Splits text into words, keeping double-quoted spans together.
///
/// Quoted spans let a builder use a multi-word name — `Call this "north
/// wall".` — without the lexer chopping it into two unrelated words.
fn split_words(text: &str) -> Vec<RawWord> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    let flush = |current: &mut String, quoted: bool, words: &mut Vec<RawWord>| {
        let trimmed = if quoted {
            current.trim().to_string()
        } else {
            current
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '=')
                .to_string()
        };
        if !trimmed.is_empty() {
            words.push(RawWord {
                text: trimmed,
                quoted,
            });
        }
        current.clear();
    };

    for c in text.chars() {
        match c {
            '"' => {
                flush(&mut current, in_quotes, &mut words);
                in_quotes = !in_quotes;
            }
            _ if c.is_whitespace() && !in_quotes => flush(&mut current, false, &mut words),
            _ => current.push(c),
        }
    }
    // An unterminated quote is treated as ordinary text.
    flush(&mut current, false, &mut words);
    words
}

fn dimension(word: &str) -> Option<(u8, u8)> {
    let (a, b) = word.split_once(['x', 'X', '×'])?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Tokenizes one sentence.
#[must_use]
pub fn tokenize_sentence(text: &str) -> Sentence {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();

    // Comment markers: everything after them is for humans only.
    let is_comment = ["note:", "todo:", "fixme:", "//"]
        .iter()
        .any(|marker| lower.starts_with(marker));
    if is_comment {
        return Sentence {
            tokens: Vec::new(),
            text: trimmed.to_string(),
            block: None,
            is_comment: true,
        };
    }

    let raw = split_words(&lower);
    let words: Vec<String> = raw.iter().map(|w| w.text.clone()).collect();

    let mut tokens = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if raw[i].quoted {
            tokens.push(Token::Quoted(words[i].clone()));
            i += 1;
            continue;
        }
        let word = &words[i];

        // Greedily match multi-word colour names such as "dark bluish grey".
        // Longest match wins, so "dark" alone never shadows "dark tan".
        let mut matched_color = None;
        let mut phrase = String::new();
        for (offset, next) in words[i..].iter().enumerate().take(4) {
            if !phrase.is_empty() {
                phrase.push(' ');
            }
            phrase.push_str(next);
            if let Some(code) = colors::code_from_name(&phrase) {
                matched_color = Some((code, offset + 1));
            } else if !colors::is_color_prefix(&phrase) {
                break;
            }
        }
        if let Some((code, len)) = matched_color {
            tokens.push(Token::Color(code));
            i += len;
            continue;
        }

        tokens.push(classify(word));
        i += 1;
    }

    Sentence {
        tokens,
        text: trimmed.to_string(),
        block: None,
        is_comment: false,
    }
}

fn classify(word: &str) -> Token {
    // Articles carry no meaning of their own, but "a"/"an" double as the
    // number one, which is how "a red brick" and "one red brick" unify.
    if let Some((w, d)) = dimension(word) {
        return Token::Dim(w, d);
    }
    if let Ok(n) = word.parse::<i64>() {
        return Token::Number(n);
    }
    if let Some(verb) = Verb::parse(word) {
        return Token::Verb(verb);
    }
    if let Some(direction) = Direction::parse(word) {
        return Token::Direction(direction);
    }
    if let Some(unit) = Unit::parse(word) {
        // Unit words double as part kinds ("brick", "plate"); the parser
        // decides which reading applies, so emit the kind when there is one.
        if let Some(kind) = PartKind::parse(word) {
            return Token::Kind(kind);
        }
        return Token::Unit(unit);
    }
    if let Some(kind) = PartKind::parse(word) {
        return Token::Kind(kind);
    }
    if let Some(keyword) = Keyword::parse(word) {
        return Token::Keyword(keyword);
    }
    if let Some(n) = number_word(word) {
        return Token::Number(n);
    }
    Token::Word(word.to_string())
}

/// Tokenizes a whole chat message into sentences.
///
/// Fenced blocks are lifted out first and attached to the sentence in front of
/// them, so their characters never reach [`strip_discord_noise`] or
/// [`split_sentences`] — both of which would destroy a drawing. Everything
/// else is unchanged: a fence never produces a sentence of its own, so sentence
/// indices mean exactly what they always did.
#[must_use]
pub fn tokenize(text: &str) -> Vec<Sentence> {
    let mut sentences: Vec<Sentence> = Vec::new();

    for segment in block::split_code_fences(text) {
        match segment {
            Segment::Text(text) => sentences.extend(
                split_sentences(&strip_discord_noise(&text))
                    .iter()
                    .map(|s| tokenize_sentence(s)),
            ),
            // A block belongs to the sentence just before it. One with nothing
            // in front of it is a pasted code sample, not a command, and one
            // sentence carries at most one drawing.
            Segment::Fence(block) => {
                if let Some(last) = sentences.last_mut() {
                    if last.block.is_none() {
                        last.block = Some(block);
                    }
                }
            }
        }
    }

    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<Token> {
        tokenize_sentence(text).tokens
    }

    #[test]
    fn recognises_verbs_and_their_synonyms() {
        assert_eq!(Verb::parse("add"), Some(Verb::Add));
        assert_eq!(Verb::parse("put"), Some(Verb::Add));
        assert_eq!(Verb::parse("delete"), Some(Verb::Remove));
        assert_eq!(Verb::parse("color"), Some(Verb::Paint));
        assert_eq!(Verb::parse("colour"), Some(Verb::Paint));
        assert_eq!(Verb::parse("banana"), None);
    }

    #[test]
    fn tokenizes_a_simple_add() {
        assert_eq!(
            tokens("Add a red 2x4 brick"),
            vec![
                Token::Verb(Verb::Add),
                Token::Number(1),
                Token::Color(4),
                Token::Dim(2, 4),
                Token::Kind(PartKind::Brick),
            ]
        );
    }

    #[test]
    fn matches_multi_word_colours_greedily() {
        assert_eq!(
            tokens("Paint it dark tan"),
            vec![
                Token::Verb(Verb::Paint),
                Token::Keyword(Keyword::It),
                Token::Color(28),
            ]
        );
        assert_eq!(
            tokens("Add a light bluish gray plate"),
            vec![
                Token::Verb(Verb::Add),
                Token::Number(1),
                Token::Color(71),
                Token::Kind(PartKind::Plate),
            ]
        );
    }

    #[test]
    fn a_colour_prefix_that_leads_nowhere_stays_a_word() {
        // "dark" on its own is not a colour, so it must not be swallowed.
        let t = tokens("Add a dark thing");
        assert!(t.contains(&Token::Word("dark".to_string())));
    }

    #[test]
    fn part_kind_words_win_over_unit_words() {
        // "brick" is both a unit of height and a kind of part.
        assert_eq!(tokens("brick"), vec![Token::Kind(PartKind::Brick)]);
        // "stud" is only ever a unit.
        assert_eq!(tokens("stud"), vec![Token::Unit(Unit::Stud)]);
    }

    #[test]
    fn number_words_and_digits_both_become_numbers() {
        assert_eq!(tokens("ten"), vec![Token::Number(10)]);
        assert_eq!(tokens("10"), vec![Token::Number(10)]);
        assert_eq!(tokens("a"), vec![Token::Number(1)]);
    }

    #[test]
    fn dimensions_accept_several_separators() {
        assert_eq!(tokens("2x4"), vec![Token::Dim(2, 4)]);
        assert_eq!(tokens("8X8"), vec![Token::Dim(8, 8)]);
        assert_eq!(tokens("1×2"), vec![Token::Dim(1, 2)]);
    }

    #[test]
    fn splits_sentences_on_terminators_and_newlines() {
        assert_eq!(
            split_sentences("Add a brick. Move it east.\nUndo."),
            vec!["Add a brick", "Move it east", "Undo"]
        );
        assert_eq!(
            split_sentences("How tall is tower?"),
            vec!["How tall is tower?"]
        );
    }

    #[test]
    fn strips_mentions_emoji_code_and_urls() {
        assert_eq!(strip_discord_noise("hey <@12345> look"), "hey look");
        assert_eq!(strip_discord_noise("see <#999> there"), "see there");
        assert_eq!(strip_discord_noise("nice <:brick:42> one"), "nice one");
        assert_eq!(strip_discord_noise("run `Add a brick.` now"), "run now");
        assert_eq!(
            strip_discord_noise("look at https://example.org/x please"),
            "look at please"
        );
        // A stray '<' with no closing '>' is kept, not eaten.
        assert!(strip_discord_noise("2 < 3").contains('<'));
    }

    #[test]
    fn code_fences_are_dropped_entirely() {
        // Still true of this function on its own. `tokenize` no longer hands
        // it a fence at all — `split_code_fences` captures those first — so a
        // drawing survives even though anything reaching here does not.
        let text = "before ```\nAdd a brick.\n``` after";
        let cleaned = strip_discord_noise(text);
        assert!(cleaned.contains("before"));
        assert!(cleaned.contains("after"));
        assert!(!cleaned.contains("Add a brick"));
    }

    #[test]
    fn a_fenced_block_is_attached_to_the_sentence_before_it() {
        let sentences = tokenize("Build from top:\n```\n..bb..\n.rbbr.\n```");
        assert_eq!(sentences.len(), 1, "a fence must not make a sentence");
        assert_eq!(sentences[0].text, "Build from top:");
        assert_eq!(
            sentences[0].block.as_ref().unwrap().lines,
            vec!["..bb..".to_string(), ".rbbr.".to_string()]
        );
    }

    #[test]
    fn a_grid_row_full_of_dots_is_not_split_into_sentences() {
        // Unfenced, a row collapses to whatever sat between its dots: '.' ends
        // a sentence, so every empty cell is erased and with it the position
        // of every full one. Capture is the only reason a drawing survives.
        assert_eq!(split_sentences("..bb.."), vec!["bb".to_string()]);
        assert_eq!(
            split_sentences(".r..r."),
            vec!["r".to_string(), "r".to_string()]
        );
        let sentences = tokenize("Build from top:\n```\n..bb..\n```");
        assert_eq!(
            sentences[0].block.as_ref().unwrap().lines,
            vec!["..bb..".to_string()]
        );
    }

    #[test]
    fn a_captured_block_keeps_its_columns_past_the_url_pass() {
        // The URL filter rejoins each line with single spaces, which would
        // shift every cell in a drawing.
        let sentences = tokenize("Build from top:\n```\n.b  b.\n```");
        assert_eq!(
            sentences[0].block.as_ref().unwrap().lines,
            vec![".b  b.".to_string()]
        );
    }

    #[test]
    fn a_captured_block_survives_the_block_quote_filter() {
        // A '>' row would otherwise be read as someone else's words quoted
        // back and dropped.
        let sentences = tokenize("Build from north:\n```\n> b <\n```");
        assert_eq!(
            sentences[0].block.as_ref().unwrap().lines,
            vec!["> b <".to_string()]
        );
    }

    #[test]
    fn a_fence_with_no_sentence_before_it_is_dropped() {
        // A pasted code sample is not a command.
        assert!(tokenize("```\n..bb..\n```").is_empty());
    }

    #[test]
    fn a_second_fence_after_the_same_sentence_is_dropped() {
        let sentences = tokenize("Build from top:\n```\naa\n```\n```\nbb\n```");
        assert_eq!(sentences.len(), 1);
        assert_eq!(
            sentences[0].block.as_ref().unwrap().lines,
            vec!["aa".to_string()]
        );
    }

    #[test]
    fn sentences_after_a_fence_are_tokenized_normally() {
        let sentences = tokenize("Build from top:\n```\nbb\n```\nHow tall is it?");
        assert_eq!(sentences.len(), 2);
        assert!(sentences[0].block.is_some());
        assert_eq!(sentences[1].text, "How tall is it?");
        assert!(sentences[1].block.is_none());
    }

    #[test]
    fn a_sentence_replays_with_its_block() {
        // What gets parked when a drawing pauses on an unknown part, and what
        // must re-lex to the very same sentence.
        let original = "Build from top:\n```\nb = blue 1x1 brick\n\n..bb..\n```";
        let parked = tokenize(original)[0].replay_text();
        let again = tokenize(&parked);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].text, "Build from top:");
        assert_eq!(again[0].block, tokenize(original)[0].block);
    }

    #[test]
    fn a_sentence_without_a_block_replays_as_itself() {
        let sentences = tokenize("Add a red 2x4 brick at origin.");
        assert_eq!(sentences[0].replay_text(), sentences[0].text);
    }

    #[test]
    fn comment_markers_short_circuit_tokenizing() {
        let s = tokenize_sentence("Note: temporary support");
        assert!(s.is_comment);
        assert!(s.tokens.is_empty());
        assert!(tokenize_sentence("TODO: replace with slope").is_comment);
        assert!(!tokenize_sentence("Add a brick").is_comment);
    }

    #[test]
    fn the_command_gate_lets_ordinary_chat_through_untouched() {
        assert!(tokenize_sentence("Add a red brick").looks_like_command());
        assert!(tokenize_sentence("Undo").looks_like_command());
        assert!(tokenize_sentence("How tall is the tower?").looks_like_command());
        assert!(tokenize_sentence("Note: careful here").looks_like_command());

        // Plain conversation must not be treated as a command.
        for chat in [
            "nice, that looks great!",
            "wow",
            "I think we should use dark tan instead",
            "the roof is my favourite bit",
            "lol",
            "brb",
            // Counts and colours on their own are how people talk about LEGO,
            // and none of them names a part.
            "2 beers",
            "3 people",
            "red",
            "blue is nicer",
            "42",
        ] {
            assert!(
                !tokenize_sentence(chat).looks_like_command(),
                "{chat:?} should have been ignored"
            );
        }
    }

    #[test]
    fn naming_a_part_on_its_own_is_something_to_act_on() {
        // Not a command yet — the parser decides that — but it must get past
        // the gate, because "2x2 brick" is plainly about building.
        for phrase in [
            "2x2 brick",
            "2 2x2 bricks stacked vertically",
            "red 2x4 brick",
            "3 plates",
            "brick",
        ] {
            assert!(
                tokenize_sentence(phrase).looks_like_command(),
                "{phrase:?} names a part and should have got through"
            );
        }
    }

    #[test]
    fn quoted_spans_survive_as_a_single_name() {
        assert_eq!(
            tokens("Call this \"north wall\""),
            vec![
                Token::Verb(Verb::Name),
                Token::Keyword(Keyword::This),
                Token::Quoted("north wall".to_string()),
            ]
        );
    }

    #[test]
    fn a_quoted_word_is_not_reinterpreted() {
        // "red" inside quotes is a name, not the colour.
        assert_eq!(
            tokens("Call this \"red\""),
            vec![
                Token::Verb(Verb::Name),
                Token::Keyword(Keyword::This),
                Token::Quoted("red".to_string()),
            ]
        );
    }

    #[test]
    fn an_unterminated_quote_is_treated_as_plain_text() {
        let t = tokens("Call this \"wall");
        assert_eq!(t.last(), Some(&Token::Word("wall".to_string())));
    }

    #[test]
    fn surrounding_punctuation_is_trimmed() {
        assert_eq!(
            tokens("Add a (red) brick,"),
            vec![
                Token::Verb(Verb::Add),
                Token::Number(1),
                Token::Color(4),
                Token::Kind(PartKind::Brick),
            ]
        );
    }

    #[test]
    fn tokenizes_a_whole_message_into_sentences() {
        let sentences = tokenize("Add a red brick. Move it east.");
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0].tokens[0], Token::Verb(Verb::Add));
        assert_eq!(sentences[1].tokens[0], Token::Verb(Verb::Move));
    }

    #[test]
    fn a_multi_line_message_is_one_sentence_per_line() {
        // Regression: `tokenize` strips Discord noise before it splits, and
        // the noise stripper used to rejoin every word with a single space.
        // Newlines vanished, so a message typed as several lines became one
        // sentence and every line after the first was silently swallowed by
        // the modifier parser.
        let sentences = tokenize("Add a red brick\nAdd a blue brick");
        assert_eq!(sentences.len(), 2, "got: {sentences:?}");
        assert_eq!(sentences[0].tokens[0], Token::Verb(Verb::Add));
        assert_eq!(sentences[1].tokens[0], Token::Verb(Verb::Add));
        // "a" lexes as the number one, so the colour is the token after it.
        assert_eq!(sentences[1].tokens[2], Token::Color(1));
    }

    #[test]
    fn urls_are_stripped_without_collapsing_the_lines_around_them() {
        assert_eq!(
            strip_discord_noise("see https://example.org/x\nAdd a brick"),
            "see\nAdd a brick"
        );
    }

    #[test]
    fn a_quoted_line_is_dropped_not_replayed() {
        // Regression: someone quoting the bot's own instructions while asking
        // "did this work?" made the bot silently re-run whatever it had said,
        // because a `> ` line reads as a perfectly ordinary sentence otherwise.
        let cleaned = strip_discord_noise("> Add a red 2x4 brick at origin.\nHow tall is it?");
        assert!(!cleaned.contains("Add"), "got: {cleaned:?}");
        assert!(cleaned.contains("How tall is it?"), "got: {cleaned:?}");
    }

    #[test]
    fn a_block_quote_drops_everything_after_it_too() {
        let cleaned = strip_discord_noise(">>> Add a red brick.\nAdd a blue brick.");
        assert!(!cleaned.contains("Add"), "got: {cleaned:?}");
    }

    #[test]
    fn indented_and_nested_quote_markers_are_still_recognised() {
        assert!(strip_discord_noise("  > Add a brick.").trim().is_empty());
        assert!(strip_discord_noise(">> Add a brick.").trim().is_empty());
    }

    #[test]
    fn a_stray_greater_than_sign_is_not_mistaken_for_a_quote() {
        // Only a '>' that opens the line is a quote marker.
        assert_eq!(
            strip_discord_noise("Count bricks taller >100 studs"),
            "Count bricks taller >100 studs"
        );
    }

    #[test]
    fn an_empty_message_yields_no_sentences() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   \n  ").is_empty());
        assert!(tokenize("`code only`").is_empty());
    }
}
