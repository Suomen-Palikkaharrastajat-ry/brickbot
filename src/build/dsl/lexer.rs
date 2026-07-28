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
    Measure,
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
            "measure" => Self::Measure,
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
}

/// One sentence's worth of tokens, plus the text it came from for error
/// messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub tokens: Vec<Token>,
    pub text: String,
    /// True when the sentence began with a comment marker such as `Note:`.
    pub is_comment: bool,
}

impl Sentence {
    /// True when the sentence opens with something the grammar could act on.
    ///
    /// This is the gate that keeps ordinary conversation out of the parser: a
    /// message that does not start with a verb or a question word is never
    /// reported as a parse error, it is simply ignored.
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
            _ => false,
        }
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

    // Drop anything that looks like a URL.
    out.split_whitespace()
        .filter(|w| !w.starts_with("http://") && !w.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ")
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
#[must_use]
pub fn tokenize(text: &str) -> Vec<Sentence> {
    let cleaned = strip_discord_noise(text);
    split_sentences(&cleaned)
        .iter()
        .map(|s| tokenize_sentence(s))
        .collect()
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
        assert_eq!(split_sentences("How tall is tower?"), vec!["How tall is tower?"]);
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
        let text = "before ```\nAdd a brick.\n``` after";
        let cleaned = strip_discord_noise(text);
        assert!(cleaned.contains("before"));
        assert!(cleaned.contains("after"));
        assert!(!cleaned.contains("Add a brick"));
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
        ] {
            assert!(
                !tokenize_sentence(chat).looks_like_command(),
                "{chat:?} should have been ignored"
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
    fn an_empty_message_yields_no_sentences() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   \n  ").is_empty());
        assert!(tokenize("`code only`").is_empty());
    }
}
