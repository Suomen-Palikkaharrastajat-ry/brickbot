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
    Command, ExtentAxis, Location, Meta, Modifier, ObjectExpr, PartSpec, Place, Query, Ref,
    Statement, Structure,
};
use super::lexer::{Keyword, Sentence, Token, Verb};
use crate::build::model::PartKind;
use crate::build::units::{Direction, IVec3, Unit};

/// Why a sentence that looked like a command could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// What the parser was looking for.
    pub expected: String,
    /// The word it stopped on, if any.
    pub found: Option<String>,
    /// The sentence as typed.
    pub sentence: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.found {
            Some(found) => write!(f, "expected {} but found '{}'", self.expected, found),
            None => write!(f, "expected {}", self.expected),
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
            bad_angle: None,
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

    fn error(&self, expected: &str) -> ParseError {
        ParseError {
            expected: expected.to_string(),
            found: self.peek().map(|t| match t {
                Token::Word(w) | Token::Quoted(w) => w.clone(),
                Token::Number(n) => n.to_string(),
                Token::Dim(w, d) => format!("{w}x{d}"),
                other => format!("{other:?}").to_lowercase(),
            }),
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

    /// Parses a part description: `[count] [colour] [size] [kind]`, in any
    /// sensible order, plus a bare part number.
    fn part_spec(&mut self) -> PartSpec {
        let mut spec = PartSpec {
            quantity: 1,
            ..PartSpec::default()
        };
        let mut saw_something = false;

        loop {
            self.skip_filler_keeping_counts();
            match self.peek() {
                Some(Token::Number(n)) => {
                    // A number here is a count, unless a dimension follows in
                    // the shape "2 x 4".
                    let value = *n;
                    if matches!(self.peek_at(1), Some(Token::Word(w)) if w == "x")
                        && matches!(self.peek_at(2), Some(Token::Number(_)))
                    {
                        self.pos += 2;
                        if let Some(Token::Number(d)) = self.peek() {
                            spec.size = Some((clamp_dim(value), clamp_dim(*d)));
                            self.pos += 1;
                            saw_something = true;
                            continue;
                        }
                    }
                    // Four digits or more is a part number, not a count:
                    // nobody asks for two thousand bricks in one sentence,
                    // and LEGO design ids are all four digits or longer.
                    if value >= PART_NUMBER_THRESHOLD && spec.part_num.is_none() {
                        spec.part_num = Some(value.to_string());
                        self.pos += 1;
                        saw_something = true;
                        continue;
                    }
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    {
                        spec.quantity = value.clamp(1, 1000) as u32;
                    }
                    self.pos += 1;
                }
                Some(Token::Color(code)) => {
                    spec.color = Some(*code);
                    self.pos += 1;
                    saw_something = true;
                }
                Some(Token::Dim(w, d)) => {
                    spec.size = Some((*w, *d));
                    self.pos += 1;
                    saw_something = true;
                }
                Some(Token::Kind(kind)) => {
                    spec.kind = Some(*kind);
                    self.pos += 1;
                    saw_something = true;
                }
                // A bare alphanumeric word containing a digit is very likely a
                // part number, e.g. "3001" or "4073b".
                Some(Token::Word(w))
                    if spec.part_num.is_none()
                        && w.chars().any(|c| c.is_ascii_digit())
                        && w.chars().all(|c| c.is_ascii_alphanumeric()) =>
                {
                    spec.part_num = Some(w.clone());
                    self.pos += 1;
                    saw_something = true;
                }
                _ => break,
            }
        }

        if !saw_something && spec.quantity == 1 {
            return PartSpec {
                quantity: 1,
                ..PartSpec::default()
            };
        }
        spec
    }

    /// Like [`Self::skip_filler`] but leaves numbers alone, for use inside a
    /// part spec where a leading count is meaningful.
    fn skip_filler_keeping_counts(&mut self) {
        while self.peek().and_then(Token::as_word) == Some("the") {
            self.pos += 1;
        }
    }

    /// Parses a reference to something already in the model.
    fn reference(&mut self) -> Option<Ref> {
        self.skip_filler();
        match self.peek() {
            Some(Token::Keyword(Keyword::That | Keyword::It | Keyword::This)) => {
                self.pos += 1;
                Some(Ref::Last)
            }
            Some(Token::Keyword(Keyword::Them | Keyword::Those | Keyword::These)) => {
                self.pos += 1;
                Some(Ref::LastPlural)
            }
            Some(Token::Keyword(Keyword::Selection)) => {
                self.pos += 1;
                Some(Ref::Selection)
            }
            Some(Token::Keyword(Keyword::Everything)) => {
                self.pos += 1;
                Some(Ref::Everything)
            }
            Some(Token::Keyword(Keyword::All)) => {
                self.pos += 1;
                let spec = self.part_spec();
                if spec.is_empty() {
                    Some(Ref::Everything)
                } else {
                    Some(Ref::AllMatching(Box::new(spec)))
                }
            }
            Some(Token::Keyword(Keyword::Last | Keyword::Previous)) => {
                self.pos += 1;
                let spec = self.part_spec();
                Some(Ref::LastOfKind(Box::new(spec)))
            }
            Some(Token::Word(w)) if w == "highest" || w == "tallest" || w == "topmost" => {
                self.pos += 1;
                let _ = self.part_spec();
                Some(Ref::Highest)
            }
            Some(Token::Color(_) | Token::Dim(_, _) | Token::Kind(_)) => {
                let spec = self.part_spec();
                if spec.is_empty() {
                    None
                } else {
                    Some(Ref::LastOfKind(Box::new(spec)))
                }
            }
            // A bare word is a group or landmark name — unless it is an
            // adverb that belongs to a trailing modifier, which would
            // otherwise be swallowed here and never reach `modifiers()`.
            Some(Token::Word(name)) if !is_modifier_word(name) => {
                let name = name.clone();
                self.pos += 1;
                Some(Ref::Named(name))
            }
            Some(Token::Quoted(name)) => {
                let name = name.clone();
                self.pos += 1;
                Some(Ref::Named(name))
            }
            _ => None,
        }
    }

    /// Parses `at ...`, `on ...`, `under ...` or `<direction> of ...`.
    fn location(&mut self) -> Option<Location> {
        let start = self.pos;
        if self.eat_keyword(Keyword::At) {
            if let Some(place) = self.place() {
                return Some(Location::At(place));
            }
            self.pos = start;
            return None;
        }

        if self.eat_keyword(Keyword::On) {
            // "on top of X" and "on X" mean the same thing.
            self.skip_filler();
            if self.eat_word("top") {
                self.eat_keyword(Keyword::Of);
            }
            if let Some(reference) = self.reference() {
                return Some(Location::On(reference));
            }
            self.pos = start;
            return None;
        }

        if self.eat_keyword(Keyword::Under) {
            if let Some(reference) = self.reference() {
                return Some(Location::Under(reference));
            }
            self.pos = start;
            return None;
        }

        if let Some(Token::Direction(direction)) = self.peek() {
            let direction = *direction;
            // Only a location when followed by "of" or "the ...".
            if matches!(self.peek_at(1), Some(Token::Keyword(Keyword::Of))) {
                self.pos += 2;
                if let Some(reference) = self.reference() {
                    return Some(Location::Direction {
                        direction,
                        of: reference,
                    });
                }
            }
            self.pos = start;
            return None;
        }

        None
    }

    fn place(&mut self) -> Option<Place> {
        self.skip_filler();
        if self.eat_keyword(Keyword::Origin) {
            return Some(Place::Origin);
        }

        // Bare coordinates: "at 12 5 3".
        if let (Some(Token::Number(x)), Some(Token::Number(y)), Some(Token::Number(z))) =
            (self.peek(), self.peek_at(1), self.peek_at(2))
        {
            #[allow(clippy::cast_possible_truncation)]
            let place = Place::Coords(IVec3::new(*x as i32, *y as i32, *z as i32));
            self.pos += 3;
            return Some(place);
        }

        // "at x=12 y=5 z=3", where each pair lexes as one word.
        if let Some(coords) = self.axis_coords() {
            return Some(Place::Coords(coords));
        }

        match self.peek() {
            Some(Token::Word(name) | Token::Quoted(name)) => {
                let name = name.clone();
                self.pos += 1;
                Some(Place::Landmark(name))
            }
            _ => None,
        }
    }

    fn axis_coords(&mut self) -> Option<IVec3> {
        let start = self.pos;
        let mut coords = IVec3::ZERO;
        let mut found = false;
        while let Some(Token::Word(w)) = self.peek() {
            let Some((axis, value)) = w.split_once('=') else {
                break;
            };
            let Ok(value) = value.parse::<i32>() else {
                break;
            };
            match axis {
                "x" => coords.x = value,
                "y" => coords.y = value,
                "z" => coords.z = value,
                _ => break,
            }
            found = true;
            self.pos += 1;
        }
        if found {
            Some(coords)
        } else {
            self.pos = start;
            None
        }
    }

    /// Parses every trailing modifier it can find.
    fn modifiers(&mut self) -> Vec<Modifier> {
        let mut modifiers = Vec::new();
        let mut guard = 0;

        while !self.at_end() && guard < 64 {
            guard += 1;
            let before = self.pos;

            // "by one plate", "by two bricks"
            if self.eat_keyword(Keyword::By) {
                if let Some(m) = self.distance_or_extent(None) {
                    modifiers.push(m);
                    continue;
                }
            }

            // "upside down"
            if self.eat_keyword(Keyword::Upside) {
                self.eat_word("down");
                modifiers.push(Modifier::Flip);
                continue;
            }

            // "90 degrees", "3 times"
            if let Some(Token::Number(n)) = self.peek() {
                let value = *n;
                if matches!(self.peek_at(1), Some(Token::Keyword(Keyword::Degrees))) {
                    self.pos += 2;
                    // Parts only sit on quarter turns, so anything else was a
                    // misunderstanding. Rounding it to zero used to fall
                    // through to the bare-`Rotate` branch and quietly turn the
                    // part 90 degrees instead.
                    if value % 90 != 0 {
                        self.bad_angle = Some(value);
                        continue;
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    modifiers.push(Modifier::Turn((value / 90) as i32));
                    continue;
                }
                if matches!(self.peek_at(1), Some(Token::Keyword(Keyword::Times))) {
                    self.pos += 2;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    modifiers.push(Modifier::Quantity(value.clamp(1, 1000) as u32));
                    continue;
                }
                // "10 studs long", "6 bricks high"
                if let Some(m) = self.distance_or_extent(None) {
                    modifiers.push(m);
                    continue;
                }
            }

            // "clockwise" / "anticlockwise"
            if self.eat_word("clockwise") {
                modifiers.push(Modifier::Turn(1));
                continue;
            }
            if self.eat_word("anticlockwise") || self.eat_word("counterclockwise") {
                modifiers.push(Modifier::Turn(-1));
                continue;
            }

            // "across the centre", "about the north-south axis" — the axis a
            // mirror reflects in. Without this the direction is swallowed by
            // the skip below and every mirror silently used the default.
            if self.eat_any_keyword(&[Keyword::Across]).is_some() {
                self.skip_filler();
                // "across the centre" names the pivot, not an axis, and the
                // default axis is already the model's centre line.
                if self.eat_keyword(Keyword::Origin) {
                    continue;
                }
                if let Some(Token::Direction(direction)) = self.peek() {
                    let direction = *direction;
                    self.pos += 1;
                    let _ = self.eat_word("axis");
                    modifiers.push(Modifier::Distance {
                        direction,
                        count: 1,
                        unit: direction.default_unit(),
                    });
                }
                continue;
            }

            // "east 3 studs", "north"
            if let Some(Token::Direction(direction)) = self.peek() {
                let direction = *direction;
                self.pos += 1;
                if let Some(m) = self.distance_or_extent(Some(direction)) {
                    modifiers.push(m);
                } else {
                    modifiers.push(Modifier::Distance {
                        direction,
                        count: 1,
                        unit: direction.default_unit(),
                    });
                }
                continue;
            }

            // "using tan plates", "with white bricks"
            // The lexer folds "using" onto `With`.
            if self.eat_any_keyword(&[Keyword::With]).is_some() {
                let spec = self.part_spec();
                if !spec.is_empty() {
                    modifiers.push(Modifier::Using(Box::new(spec)));
                    continue;
                }
            }

            if self.pos == before {
                // Nothing matched; skip the token so we always terminate.
                self.pos += 1;
            }
        }

        modifiers
    }

    /// Parses `<count> <unit> [long|high|wide|deep]`, producing either a
    /// distance along `direction` or an extent.
    fn distance_or_extent(&mut self, direction: Option<Direction>) -> Option<Modifier> {
        let start = self.pos;
        let Some(count) = self.number() else {
            self.pos = start;
            return None;
        };

        let unit = match self.peek() {
            Some(Token::Unit(u)) => {
                let u = *u;
                self.pos += 1;
                Some(u)
            }
            // "6 bricks high" lexes "bricks" as a part kind, not a unit.
            Some(Token::Kind(PartKind::Brick)) => {
                self.pos += 1;
                Some(Unit::Brick)
            }
            Some(Token::Kind(PartKind::Plate)) => {
                self.pos += 1;
                Some(Unit::Plate)
            }
            _ => None,
        };

        let axis = match self.peek() {
            Some(Token::Keyword(Keyword::Long)) => Some(ExtentAxis::Length),
            Some(Token::Keyword(Keyword::High | Keyword::Tall)) => Some(ExtentAxis::Height),
            Some(Token::Keyword(Keyword::Wide)) => Some(ExtentAxis::Width),
            Some(Token::Keyword(Keyword::Deep)) => Some(ExtentAxis::Depth),
            _ => None,
        };

        if let Some(axis) = axis {
            self.pos += 1;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            return Some(Modifier::Extent {
                axis,
                count: count.clamp(1, 1000) as u32,
                unit: unit.unwrap_or(match axis {
                    ExtentAxis::Height => Unit::Brick,
                    _ => Unit::Stud,
                }),
            });
        }

        // A direction after the count also works: "3 studs east".
        let direction = direction.or_else(|| match self.peek() {
            Some(Token::Direction(d)) => {
                let d = *d;
                self.pos += 1;
                Some(d)
            }
            _ => None,
        });

        match direction {
            Some(direction) =>
            {
                #[allow(clippy::cast_possible_truncation)]
                Some(Modifier::Distance {
                    direction,
                    count: count as i32,
                    unit: unit.unwrap_or_else(|| direction.default_unit()),
                })
            }
            None if unit.is_some() => {
                // A bare "by one plate" with no direction means upwards, which
                // is what "raise"/"lower" supply from the verb.
                #[allow(clippy::cast_possible_truncation)]
                Some(Modifier::Distance {
                    direction: Direction::Up,
                    count: count as i32,
                    unit: unit.unwrap_or(Unit::Brick),
                })
            }
            None => {
                self.pos = start;
                None
            }
        }
    }
}

/// Parses one sentence.
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
                _ => SentenceOutcome::Error(parser.error("a revision number")),
            }
        }
        Verb::Export => SentenceOutcome::Statement(Statement::Meta(Meta::Export)),
        Verb::Render => SentenceOutcome::Statement(Statement::Meta(Meta::Render)),
        Verb::Help => SentenceOutcome::Statement(Statement::Meta(Meta::Help)),
        Verb::Finish => SentenceOutcome::Statement(Statement::Meta(Meta::Finish)),
        Verb::Count => {
            let spec = parser.part_spec();
            SentenceOutcome::Statement(Statement::Query(Query::Count(Box::new(spec))))
        }
        Verb::Find => {
            if parser.eat_word("collisions") {
                return SentenceOutcome::Statement(Statement::Query(Query::FindCollisions));
            }
            SentenceOutcome::Error(parser.error("something to find, such as 'collisions'"))
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
            SentenceOutcome::Error(parser.error("something to show, such as 'parts'"))
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

fn parse_query(parser: &mut Parser<'_>) -> Option<Query> {
    let start = parser.pos;

    if parser.eat_word("how") {
        // "tall" and "high" lex as keywords, because they also end an extent
        // modifier ("6 bricks high").
        if parser.eat_keyword(Keyword::Tall) || parser.eat_keyword(Keyword::High) {
            parser.eat_word("is");
            let reference = parser.reference();
            return Some(Query::HowTall(reference));
        }
        if parser.eat_word("many") {
            let spec = parser.part_spec();
            return Some(Query::Count(Box::new(spec)));
        }
        parser.pos = start;
        return None;
    }

    // "where" and "who" need the rest of their shape before they count as a
    // question about the model. Without that, "who wants to help?" parses as
    // "who added <a thing called 'wants'>" and the thread gets an error about
    // a group nobody named.
    if parser.eat_word("where") {
        if !(parser.eat_word("is") || parser.eat_word("are")) {
            parser.pos = start;
            return None;
        }
        let Some(reference) = parser.reference() else {
            parser.pos = start;
            return None;
        };
        return Some(Query::WhereIs(reference));
    }

    if parser.eat_word("who") {
        if !(parser.eat_word("added")
            || parser.eat_word("built")
            || parser.eat_word("made")
            || parser.eat_word("placed"))
        {
            parser.pos = start;
            return None;
        }
        let Some(reference) = parser.reference() else {
            parser.pos = start;
            return None;
        };
        return Some(Query::WhoAdded(reference));
    }

    if parser.eat_word("what") {
        if parser.eat_word("changed") {
            return Some(Query::WhatChanged);
        }
        parser.pos = start;
        return None;
    }

    parser.pos = start;
    None
}

fn parse_name(parser: &mut Parser<'_>) -> SentenceOutcome {
    // "Call this wall." / "Name this point gate." / "Call the current
    // assembly castle."
    parser.skip_filler();
    // "this"/"that"/"it" just points at whatever was last touched, which is
    // already the default subject, so it needs no further handling.
    let _ = parser.eat_keyword(Keyword::This)
        || parser.eat_keyword(Keyword::That)
        || parser.eat_keyword(Keyword::It);
    parser.skip_filler();
    parser.eat_keyword(Keyword::Current);
    parser.skip_filler();
    // "Name this point gate." names a location; everything else names the
    // parts themselves.
    let naming_point = parser.eat_keyword(Keyword::Point);
    let _ = parser.eat_word("assembly") || parser.eat_word("model") || parser.eat_word("group");

    parser.skip_filler();
    let name = match parser.next() {
        Some(Token::Word(w) | Token::Quoted(w)) => w.clone(),
        _ => return SentenceOutcome::Error(parser.error("a name")),
    };

    let mut command = Command {
        verb: Verb::Name,
        object: ObjectExpr::Reference(Ref::Last),
        location: None,
        modifiers: Vec::new(),
        target_color: None,
        target_part: None,
    };
    if naming_point {
        command.modifiers.push(Modifier::Quantity(0));
    }
    command.target_part = Some(Box::new(PartSpec {
        part_num: Some(name),
        ..PartSpec::default()
    }));
    SentenceOutcome::Statement(Statement::Command(Box::new(command)))
}

fn parse_command(parser: &mut Parser<'_>, verb: Verb) -> SentenceOutcome {
    let mut target_color = None;
    let mut target_part = None;
    // Modifiers recognised while parsing the object, which must survive into
    // the final command alongside the trailing ones.
    let mut extra_modifiers: Vec<Modifier> = Vec::new();

    let object = match verb {
        Verb::Build => {
            parser.skip_filler();
            // "Build a wall ..." / "Build a row of 20 black tiles east."
            let structure = match parser.peek() {
                Some(Token::Word(w)) => {
                    Structure::parse(w).unwrap_or_else(|| Structure::Unknown(w.clone()))
                }
                Some(Token::Keyword(Keyword::Steps)) => Structure::Stairs,
                _ => return SentenceOutcome::Error(parser.error("something to build")),
            };
            parser.pos += 1;

            // "Build a row of 20 black tiles east." — the `of` clause carries
            // both how many and what to build it from.
            if parser.eat_keyword(Keyword::Of) {
                let spec = parser.part_spec();
                if !spec.is_empty() {
                    extra_modifiers.push(Modifier::Using(Box::new(spec)));
                }
            }
            ObjectExpr::Structure(structure)
        }
        Verb::Add | Verb::Fill => {
            let spec = parser.part_spec();
            if spec.is_empty() {
                return SentenceOutcome::Error(parser.error("a part to add"));
            }
            ObjectExpr::Part(Box::new(spec))
        }
        Verb::Paint => {
            // The colour always comes last: "Paint selection dark tan.",
            // "Paint all red bricks blue." Claim it before parsing the object,
            // or the part description would absorb it.
            let Some(color_index) = parser.last_color_index(parser.pos) else {
                return SentenceOutcome::Error(parser.error("a colour to paint with"));
            };
            let Some(Token::Color(code)) = parser.tokens.get(color_index) else {
                return SentenceOutcome::Error(parser.error("a colour to paint with"));
            };
            target_color = Some(*code);

            let full_end = parser.end;
            parser.end = color_index;
            let reference = parser.reference().unwrap_or(Ref::Selection);
            parser.end = full_end;
            parser.pos = color_index + 1;

            ObjectExpr::Reference(reference)
        }
        Verb::Replace => {
            let Some(reference) = parser.reference() else {
                return SentenceOutcome::Error(parser.error("something to replace"));
            };
            parser.eat_keyword(Keyword::With);
            parser.skip_filler();
            if let Some(Token::Color(code)) = parser.peek() {
                // "Replace all red bricks with dark red." is a repaint.
                target_color = Some(*code);
                parser.pos += 1;
            } else {
                let spec = parser.part_spec();
                if spec.is_empty() {
                    return SentenceOutcome::Error(parser.error("a replacement part or colour"));
                }
                target_part = Some(Box::new(spec));
            }
            ObjectExpr::Reference(reference)
        }
        Verb::Select => {
            let Some(reference) = parser.reference() else {
                return SentenceOutcome::Error(parser.error("something to select"));
            };
            ObjectExpr::Reference(reference)
        }
        Verb::Group => {
            let reference = parser.reference().unwrap_or(Ref::Selection);
            parser.eat_keyword(Keyword::To);
            let _ = parser.eat_word("as");
            parser.skip_filler();
            match parser.next() {
                Some(Token::Word(w) | Token::Quoted(w)) => {
                    target_part = Some(Box::new(PartSpec {
                        part_num: Some(w.clone()),
                        ..PartSpec::default()
                    }));
                }
                _ => return SentenceOutcome::Error(parser.error("a group name")),
            }
            ObjectExpr::Reference(reference)
        }
        Verb::Move
        | Verb::Remove
        | Verb::Rotate
        | Verb::Raise
        | Verb::Lower
        | Verb::Mirror
        | Verb::Duplicate
        | Verb::Attach
        | Verb::Repeat => {
            let reference = parser.reference().unwrap_or(Ref::Last);
            ObjectExpr::Reference(reference)
        }
        _ => ObjectExpr::None,
    };

    let location = parser.location();
    let mut modifiers = extra_modifiers;
    modifiers.extend(parser.modifiers());

    if let Some(angle) = parser.bad_angle {
        return SentenceOutcome::Error(ParseError {
            expected: "a multiple of 90 degrees".to_string(),
            found: Some(format!("{angle} degrees")),
            sentence: parser.sentence.to_string(),
        });
    }

    // "Raise by one plate." and "Lower by two bricks." carry their direction
    // in the verb, so rewrite the bare distance the parser produced.
    match verb {
        Verb::Raise | Verb::Lower => {
            let direction = if verb == Verb::Raise {
                Direction::Up
            } else {
                Direction::Down
            };
            if let Some(Modifier::Distance { count, unit, .. }) = modifiers
                .iter()
                .find(|m| matches!(m, Modifier::Distance { .. }))
                .cloned()
            {
                modifiers.retain(|m| !matches!(m, Modifier::Distance { .. }));
                modifiers.push(Modifier::Distance {
                    direction,
                    count,
                    unit,
                });
            } else {
                modifiers.push(Modifier::Distance {
                    direction,
                    count: 1,
                    unit: Unit::Brick,
                });
            }
        }
        _ => {}
    }

    SentenceOutcome::Statement(Statement::Command(Box::new(Command {
        verb,
        object,
        location,
        modifiers,
        target_color,
        target_part,
    })))
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

/// Numbers at or above this are read as part numbers rather than counts.
const PART_NUMBER_THRESHOLD: i64 = 1000;

/// Words that only ever belong to a trailing modifier, so a reference must
/// never consume them as a name.
fn is_modifier_word(word: &str) -> bool {
    matches!(
        word,
        "clockwise" | "anticlockwise" | "counterclockwise" | "down" | "around"
    )
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const fn clamp_dim(value: i64) -> u8 {
    if value < 1 {
        1
    } else if value > 255 {
        255
    } else {
        value as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::dsl::lexer::tokenize_sentence;

    fn parse(text: &str) -> SentenceOutcome {
        parse_sentence(&tokenize_sentence(text))
    }

    fn command(text: &str) -> Command {
        match parse(text) {
            SentenceOutcome::Statement(Statement::Command(c)) => *c,
            other => panic!("expected a command from {text:?}, got {other:?}"),
        }
    }

    fn statement(text: &str) -> Statement {
        match parse(text) {
            SentenceOutcome::Statement(s) => s,
            other => panic!("expected a statement from {text:?}, got {other:?}"),
        }
    }

    // --- The ignore policy -------------------------------------------------

    #[test]
    fn ordinary_chat_is_ignored_silently() {
        for chat in [
            "nice, that looks great!",
            "wow that's cool",
            "I reckon the roof needs work",
            "brb",
            "haha",
            "yeah agreed",
            "the tower is looking good",
        ] {
            assert_eq!(
                parse(chat),
                SentenceOutcome::Ignored,
                "{chat:?} should be ignored"
            );
        }
    }

    #[test]
    fn a_question_that_is_not_about_the_model_is_ignored() {
        // Regression: `looks_like_command` lets anything opening with a
        // question word through so real queries can be parsed, but a sentence
        // that then fails to parse as a query used to be *reported*. Ordinary
        // curiosity in the thread got a ❓ and a reply.
        // "where is everyone?" is deliberately absent: it has exactly the
        // shape of "where is tower?", so the parser cannot tell them apart and
        // must not try. It is silenced a stage later, when the name fails to
        // resolve — see `dispatch`.
        for chat in [
            "what do you think?",
            "who wants to help with the roof?",
            "how is it going?",
            "which one looks better?",
            "what about a bigger gate?",
            "who is building the tower?",
        ] {
            assert_eq!(
                parse(chat),
                SentenceOutcome::Ignored,
                "{chat:?} should be ignored"
            );
        }
    }

    #[test]
    fn a_real_question_is_still_answered() {
        // The silence above must not swallow the queries the language does
        // know, which open with the same words.
        for question in [
            "How tall is it?",
            "How many red bricks?",
            "What changed?",
            "Who added roof?",
            "Where is tower?",
        ] {
            assert!(
                matches!(
                    parse(question),
                    SentenceOutcome::Statement(Statement::Query(_))
                ),
                "{question:?} should be a query, got {:?}",
                parse(question)
            );
        }
    }

    #[test]
    fn an_angle_that_is_not_a_quarter_turn_is_rejected() {
        // Regression: 45/90 truncated to zero turns, which then fell into the
        // "a bare Rotate means one quarter turn" branch — so asking for 45
        // degrees silently turned the part 90.
        for sentence in ["Rotate 45 degrees", "Rotate it 30 degrees"] {
            let SentenceOutcome::Error(error) = parse(sentence) else {
                panic!("{sentence:?} should be rejected, got {:?}", parse(sentence));
            };
            assert!(error.expected.contains("90"));
        }
        // The ones that do land on a quarter turn still work.
        assert!(
            command("Rotate 90 degrees")
                .modifiers
                .contains(&Modifier::Turn(1))
        );
        assert!(
            command("Rotate 270 degrees")
                .modifiers
                .contains(&Modifier::Turn(3))
        );
    }

    #[test]
    fn a_sentence_opening_with_a_verb_is_still_reported_when_it_fails() {
        // The other half of the policy: a builder who clearly meant something
        // must be told, rather than ignored.
        assert!(matches!(parse("Add the"), SentenceOutcome::Error(_)));
        assert!(matches!(parse("Find"), SentenceOutcome::Error(_)));
    }

    #[test]
    fn comments_are_acknowledged_but_not_acted_on() {
        assert_eq!(
            parse("Note: temporary support"),
            SentenceOutcome::Statement(Statement::Comment)
        );
        assert_eq!(
            parse("TODO: replace with slope"),
            SentenceOutcome::Statement(Statement::Comment)
        );
    }

    #[test]
    fn a_sentence_that_starts_with_a_verb_but_makes_no_sense_is_an_error() {
        assert!(matches!(parse("Add the"), SentenceOutcome::Error(_)));
        assert!(matches!(parse("Replace"), SentenceOutcome::Error(_)));
    }

    // --- Adding parts ------------------------------------------------------

    #[test]
    fn parses_a_simple_add() {
        let c = command("Add a red 2x4 brick");
        assert_eq!(c.verb, Verb::Add);
        match c.object {
            ObjectExpr::Part(spec) => {
                assert_eq!(spec.color, Some(4));
                assert_eq!(spec.size, Some((2, 4)));
                assert_eq!(spec.kind, Some(PartKind::Brick));
                assert_eq!(spec.quantity, 1);
            }
            other => panic!("expected a part, got {other:?}"),
        }
        assert!(c.location.is_none());
    }

    #[test]
    fn parses_an_add_at_the_origin() {
        let c = command("Add a red 2x4 brick at origin");
        assert_eq!(c.location, Some(Location::At(Place::Origin)));
    }

    #[test]
    fn parses_relative_placement() {
        let c = command("Add a red 2x4 brick on the blue plate");
        assert!(matches!(c.location, Some(Location::On(Ref::LastOfKind(_)))));

        let c = command("Put a blue 2x2 plate on top of that");
        assert_eq!(c.location, Some(Location::On(Ref::Last)));

        let c = command("Add a black tile under the roof");
        assert_eq!(
            c.location,
            Some(Location::Under(Ref::Named("roof".to_string())))
        );
    }

    #[test]
    fn parses_a_compass_relative_location() {
        let c = command("Add a white 1x2 plate north of the window");
        assert_eq!(
            c.location,
            Some(Location::Direction {
                direction: Direction::North,
                of: Ref::Named("window".to_string()),
            })
        );
    }

    #[test]
    fn parses_explicit_coordinates() {
        assert_eq!(
            command("Add a red brick at 12 5 3").location,
            Some(Location::At(Place::Coords(IVec3::new(12, 5, 3))))
        );
        assert_eq!(
            command("Add a red brick at x=12 y=5 z=3").location,
            Some(Location::At(Place::Coords(IVec3::new(12, 5, 3))))
        );
        assert_eq!(
            command("Add a red brick at gate").location,
            Some(Location::At(Place::Landmark("gate".to_string())))
        );
    }

    #[test]
    fn parses_a_bare_part_number() {
        let c = command("Add 3001 at origin");
        match c.object {
            ObjectExpr::Part(spec) => assert_eq!(spec.part_num.as_deref(), Some("3001")),
            other => panic!("expected a part, got {other:?}"),
        }
    }

    // --- Moving and rotating ----------------------------------------------

    #[test]
    fn parses_movement_with_and_without_units() {
        let c = command("Move the red brick one stud east");
        assert_eq!(c.distance(), Some((Direction::East, 1, Unit::Stud)));

        let c = command("Move east 3 studs");
        assert_eq!(c.distance(), Some((Direction::East, 3, Unit::Stud)));

        // A bare direction means one step in that direction's default unit.
        let c = command("Move it west");
        assert_eq!(c.distance(), Some((Direction::West, 1, Unit::Stud)));
    }

    #[test]
    fn raise_and_lower_supply_their_own_direction() {
        assert_eq!(
            command("Raise by one plate").distance(),
            Some((Direction::Up, 1, Unit::Plate))
        );
        assert_eq!(
            command("Lower by two bricks").distance(),
            Some((Direction::Down, 2, Unit::Brick))
        );
        assert_eq!(
            command("Raise it").distance(),
            Some((Direction::Up, 1, Unit::Brick))
        );
    }

    #[test]
    fn parses_rotations() {
        assert!(
            command("Rotate 90 degrees")
                .modifiers
                .contains(&Modifier::Turn(1))
        );
        assert!(
            command("Rotate 180 degrees")
                .modifiers
                .contains(&Modifier::Turn(2))
        );
        assert!(
            command("Rotate clockwise")
                .modifiers
                .contains(&Modifier::Turn(1))
        );
        assert!(
            command("Turn it upside down")
                .modifiers
                .contains(&Modifier::Flip)
        );
    }

    // --- References --------------------------------------------------------

    #[test]
    fn parses_conversational_references() {
        assert_eq!(
            command("Move it east").object,
            ObjectExpr::Reference(Ref::Last)
        );
        assert_eq!(
            command("Move them east").object,
            ObjectExpr::Reference(Ref::LastPlural)
        );
        match command("Remove the last brick").object {
            ObjectExpr::Reference(Ref::LastOfKind(spec)) => {
                assert_eq!(spec.kind, Some(PartKind::Brick));
            }
            other => panic!("expected last-of-kind, got {other:?}"),
        }
    }

    #[test]
    fn parses_selections() {
        let c = command("Select all red bricks");
        match c.object {
            ObjectExpr::Reference(Ref::AllMatching(spec)) => {
                assert_eq!(spec.color, Some(4));
                assert_eq!(spec.kind, Some(PartKind::Brick));
            }
            other => panic!("expected all-matching, got {other:?}"),
        }

        assert_eq!(
            command("Select everything").object,
            ObjectExpr::Reference(Ref::Everything)
        );
        assert_eq!(
            command("Select the highest brick").object,
            ObjectExpr::Reference(Ref::Highest)
        );
    }

    #[test]
    fn parses_painting() {
        let c = command("Paint selection dark tan");
        assert_eq!(c.verb, Verb::Paint);
        assert_eq!(c.target_color, Some(28));
        assert_eq!(c.object, ObjectExpr::Reference(Ref::Selection));

        let c = command("Paint it blue");
        assert_eq!(c.target_color, Some(1));
    }

    #[test]
    fn parses_replacement_of_both_colours_and_parts() {
        let c = command("Replace all red bricks with dark red");
        assert_eq!(c.target_color, Some(320));

        let c = command("Replace 2x4 bricks with 2x2 bricks");
        assert_eq!(c.target_part.as_ref().and_then(|s| s.size), Some((2, 2)));
    }

    // --- High-level structures --------------------------------------------

    #[test]
    fn parses_a_wall_with_extents() {
        let c = command("Build a wall 10 studs long 6 bricks high");
        assert_eq!(c.object, ObjectExpr::Structure(Structure::Wall));
        assert_eq!(c.extent(ExtentAxis::Length), Some((10, Unit::Stud)));
        assert_eq!(c.extent(ExtentAxis::Height), Some((6, Unit::Brick)));
    }

    #[test]
    fn parses_a_row_and_a_floor() {
        let c = command("Build a row of 20 black tiles east");
        assert_eq!(c.object, ObjectExpr::Structure(Structure::Row));
        assert_eq!(c.distance().map(|(d, ..)| d), Some(Direction::East));

        let c = command("Build a floor 10 by 8 using tan plates");
        assert_eq!(c.object, ObjectExpr::Structure(Structure::Floor));
        assert_eq!(c.using().and_then(|s| s.kind), Some(PartKind::Plate));
    }

    #[test]
    fn an_unknown_structure_is_kept_so_the_bot_can_say_so() {
        let c = command("Build a gazebo");
        assert_eq!(
            c.object,
            ObjectExpr::Structure(Structure::Unknown("gazebo".to_string()))
        );
    }

    // --- Meta and queries --------------------------------------------------

    #[test]
    fn parses_history_commands() {
        assert_eq!(statement("Undo"), Statement::Meta(Meta::Undo));
        assert_eq!(statement("Redo"), Statement::Meta(Meta::Redo));
        assert_eq!(
            statement("Revert to revision 200"),
            Statement::Meta(Meta::RevertTo(200))
        );
        assert_eq!(
            statement("Undo alice's last edit"),
            Statement::Meta(Meta::UndoBy("alice".to_string()))
        );
    }

    #[test]
    fn parses_session_commands() {
        assert_eq!(statement("Export"), Statement::Meta(Meta::Export));
        assert_eq!(statement("Render"), Statement::Meta(Meta::Render));
        assert_eq!(statement("Help"), Statement::Meta(Meta::Help));
        assert_eq!(statement("Finish"), Statement::Meta(Meta::Finish));
    }

    #[test]
    fn parses_queries() {
        assert_eq!(
            statement("How tall is tower?"),
            Statement::Query(Query::HowTall(Some(Ref::Named("tower".to_string()))))
        );
        assert_eq!(
            statement("Find collisions"),
            Statement::Query(Query::FindCollisions)
        );
        assert_eq!(
            statement("Show unsupported bricks"),
            Statement::Query(Query::ShowUnsupported)
        );
        assert_eq!(
            statement("What changed?"),
            Statement::Query(Query::WhatChanged)
        );
        assert_eq!(
            statement("Who added roof?"),
            Statement::Query(Query::WhoAdded(Ref::Named("roof".to_string())))
        );
    }

    #[test]
    fn parses_counting_both_ways() {
        assert!(matches!(
            statement("Count red bricks"),
            Statement::Query(Query::Count(_))
        ));
        assert!(matches!(
            statement("How many red bricks?"),
            Statement::Query(Query::Count(_))
        ));
    }

    #[test]
    fn parses_naming() {
        let c = command("Call this wall");
        assert_eq!(c.verb, Verb::Name);
        assert_eq!(
            c.target_part.as_ref().and_then(|s| s.part_num.clone()),
            Some("wall".to_string())
        );

        let c = command("Name this point gate");
        assert_eq!(
            c.target_part.as_ref().and_then(|s| s.part_num.clone()),
            Some("gate".to_string())
        );
    }

    // --- Whole messages ----------------------------------------------------

    #[test]
    fn a_multi_sentence_message_parses_in_order() {
        let outcomes = parse_message("Add a red 2x4 brick. Move it east. Undo.");
        assert_eq!(outcomes.len(), 3);
        assert!(matches!(
            outcomes[0],
            SentenceOutcome::Statement(Statement::Command(_))
        ));
        assert!(matches!(
            outcomes[2],
            SentenceOutcome::Statement(Statement::Meta(Meta::Undo))
        ));
    }

    #[test]
    fn chat_mixed_with_a_command_only_acts_on_the_command() {
        let outcomes = parse_message("nice one! Add a red brick.");
        assert_eq!(outcomes[0], SentenceOutcome::Ignored);
        assert!(matches!(
            outcomes[1],
            SentenceOutcome::Statement(Statement::Command(_))
        ));
    }

    #[test]
    fn parsing_always_terminates_on_adversarial_input() {
        // Long runs of unrecognised tokens must not loop forever.
        let text = format!("Add {}", "blah ".repeat(200));
        let _ = parse(&text);
        let text = format!("Move it {}", "east ".repeat(100));
        let _ = parse(&text);
    }
}
