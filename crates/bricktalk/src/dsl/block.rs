//! Lifting fenced code blocks out of a message before anything else reads it.
//!
//! A drawing typed into a build thread is an ASCII grid, and every other stage
//! of the lexer would destroy one: `.` and newlines both end sentences, the URL
//! pass rejoins each line on single spaces, and the block-quote filter drops
//! lines beginning `>`. So a fence is captured **first**, verbatim, and the
//! rest of the lexer never sees its characters.
//!
//! Only a run of three or more backticks opens a captured fence. One or two
//! stay what they have always been: inline code, dropped as noise.

#![allow(clippy::doc_markdown)]

/// A fenced code block, captured exactly as it was typed.
///
/// Column alignment is the whole point of a drawing, so lines are stored with
/// their interior spacing untouched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodeBlock {
    pub lines: Vec<String>,
}

impl CodeBlock {
    /// Builds a block from the text between an opening and closing fence.
    fn from_body(body: &str) -> Self {
        // A fence spanning several lines opens with an info string — the
        // `text` in ```text — which is markup, not content. A single-line
        // fence has no info string, so all of it is content.
        let body = body.find('\n').map_or(body, |nl| &body[nl + 1..]);

        let mut lines: Vec<String> = body
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();

        // Blank lines separate layers, so interior ones are meaningful and
        // only the outermost are padding around the fence markers.
        let blank = |line: &String| line.trim().is_empty();
        let start = lines.iter().position(|l| !blank(l));
        let Some(start) = start else {
            return Self { lines: Vec::new() };
        };
        let end = lines.iter().rposition(|l| !blank(l)).unwrap_or(start);
        lines.truncate(end + 1);
        lines.drain(..start);

        Self { lines }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// One piece of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Ordinary text, still to be noise-stripped and split into sentences.
    Text(String),
    Fence(CodeBlock),
}

/// The next run of three or more backticks, as `(start, end)` byte offsets.
///
/// Shorter runs are stepped over rather than matched, which is what leaves
/// inline code to the noise stripper.
fn find_fence(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            if i - start >= 3 {
                return Some((start, i));
            }
        } else {
            i += 1;
        }
    }
    None
}

fn push_text(segments: &mut Vec<Segment>, text: &str) {
    if !text.is_empty() {
        segments.push(Segment::Text(text.to_string()));
    }
}

/// Splits a message into ordinary text and captured fences, in order.
#[must_use]
pub fn split_code_fences(text: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut rest = text;

    while let Some((open_start, open_end)) = find_fence(rest) {
        push_text(&mut segments, &rest[..open_start]);
        let after = &rest[open_end..];
        let (body, tail) = find_fence(after).map_or((after, ""), |(close_start, close_end)| {
            (&after[..close_start], &after[close_end..])
        });
        segments.push(Segment::Fence(CodeBlock::from_body(body)));
        rest = tail;
    }

    push_text(&mut segments, rest);
    segments
}

/// What one line inside a block turns out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineShape<'a> {
    /// `b = blue 1x1 brick`, with the description still unparsed.
    Legend { symbol: char, spec: &'a str },
    /// An `xx = …` line whose key was not a single character. Worth telling
    /// the builder about, because otherwise every cell of the drawing comes
    /// back as an unknown symbol instead.
    BadLegendKey(&'a str),
    /// A blank line, which separates one layer from the next.
    Blank,
    /// A row of the drawing.
    Row,
}

/// Classifies one line of a block.
///
/// A legend line is recognised by its shape rather than its position, so the
/// legend may sit above, below or among the drawing.
#[must_use]
pub fn legend_line(line: &str) -> LineShape<'_> {
    if line.trim().is_empty() {
        return LineShape::Blank;
    }
    let trimmed = line.trim_start();

    // The key is the first character, and the next non-space one must be `=`.
    // Reading it this way rather than splitting on the first `=` is what lets
    // a legend define `=` itself: `= = red 1x1 tile`.
    if let Some(symbol) = trimmed.chars().next() {
        let rest = &trimmed[symbol.len_utf8()..];
        if let Some(spec) = rest.trim_start().strip_prefix('=') {
            let spec = spec.trim();
            return if spec.is_empty() {
                LineShape::Row
            } else {
                LineShape::Legend { symbol, spec }
            };
        }
    }

    // Not a single-character key. If the left side reads like one anyway —
    // `bb = blue 1x1 brick` — say so, because otherwise every cell of the
    // drawing comes back as an unknown symbol instead. A row that merely
    // contains `=`, such as `..==..`, has punctuation on the left and is left
    // alone.
    if let Some((key, spec)) = trimmed.split_once('=') {
        let key = key.trim();
        if !spec.trim().is_empty()
            && key.chars().count() > 1
            && key.chars().all(char::is_alphanumeric)
        {
            return LineShape::BadLegendKey(key);
        }
    }
    LineShape::Row
}

/// A block sorted into its legend and its layers.
///
/// The part descriptions are still text: turning them into [`PartSpec`]s needs
/// the parser, which sits a layer above this one.
///
/// [`PartSpec`]: super::ast::PartSpec
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockShape {
    pub legend: Vec<(char, String)>,
    /// Each layer's rows, with trailing whitespace removed — it is invisible
    /// to whoever typed it, and editors add it unbidden.
    pub layers: Vec<Vec<Vec<char>>>,
    /// The first badly-keyed legend line, if there was one.
    pub bad_legend_key: Option<String>,
}

/// Sorts a captured block into its legend and its layers.
#[must_use]
pub fn shape(block: &CodeBlock) -> BlockShape {
    let mut shaped = BlockShape::default();
    let mut current: Vec<Vec<char>> = Vec::new();

    for line in &block.lines {
        match legend_line(line) {
            LineShape::Legend { symbol, spec } => {
                shaped.legend.push((symbol, spec.to_string()));
            }
            LineShape::BadLegendKey(key) => {
                if shaped.bad_legend_key.is_none() {
                    shaped.bad_legend_key = Some(key.to_string());
                }
            }
            LineShape::Blank => {
                // A blank line closes a layer. Runs of them, and the blank
                // that usually separates a legend from the drawing, must not
                // produce empty layers.
                if !current.is_empty() {
                    shaped.layers.push(std::mem::take(&mut current));
                }
            }
            LineShape::Row => current.push(line.trim_end().chars().collect()),
        }
    }

    if !current.is_empty() {
        shaped.layers.push(current);
    }
    shaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fences(text: &str) -> Vec<Vec<String>> {
        split_code_fences(text)
            .into_iter()
            .filter_map(|s| match s {
                Segment::Fence(b) => Some(b.lines),
                Segment::Text(_) => None,
            })
            .collect()
    }

    fn texts(text: &str) -> Vec<String> {
        split_code_fences(text)
            .into_iter()
            .filter_map(|s| match s {
                Segment::Text(t) => Some(t),
                Segment::Fence(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_triple_backtick_fence_is_lifted_out_whole() {
        let text = "Build from top:\n```\n..bb..\n.rbbr.\n```";
        assert_eq!(
            fences(text),
            vec![vec!["..bb..".to_string(), ".rbbr.".to_string()]]
        );
        assert_eq!(texts(text), vec!["Build from top:\n".to_string()]);
    }

    #[test]
    fn an_inline_code_span_is_not_a_fence() {
        // One and two backticks stay noise for the stripper to drop, which is
        // what keeps `run `Add a brick.` now` behaving as it always has.
        assert!(fences("run `Add a brick.` now").is_empty());
        assert!(fences("a ``b`` c").is_empty());
        assert_eq!(
            texts("run `Add a brick.` now"),
            vec!["run `Add a brick.` now".to_string()]
        );
    }

    #[test]
    fn a_fence_keeps_its_newlines_and_column_alignment() {
        // The reason capture has to happen before anything else: every other
        // stage would collapse this.
        let text = "Build from top:\n```\n..bb..\n.r  r.\n```";
        assert_eq!(
            fences(text)[0],
            vec!["..bb..".to_string(), ".r  r.".to_string()]
        );
    }

    #[test]
    fn an_info_string_after_the_opening_fence_is_dropped() {
        let text = "Build from top:\n```text\n..bb..\n```";
        assert_eq!(fences(text)[0], vec!["..bb..".to_string()]);
    }

    #[test]
    fn a_single_line_fence_keeps_its_only_line() {
        assert_eq!(fences("say ```xx``` now")[0], vec!["xx".to_string()]);
    }

    #[test]
    fn an_unterminated_fence_runs_to_the_end_of_the_message() {
        // Someone who forgot the closing line still meant to draw something.
        let text = "Build from top:\n```\n..bb..\n.rbbr.";
        assert_eq!(
            fences(text)[0],
            vec!["..bb..".to_string(), ".rbbr.".to_string()]
        );
    }

    #[test]
    fn blank_lines_around_a_fence_are_trimmed_but_interior_ones_are_kept() {
        // Interior blanks separate layers, so they carry meaning.
        let text = "```\n\nbb\n\ncc\n\n```";
        assert_eq!(
            fences(text)[0],
            vec!["bb".to_string(), String::new(), "cc".to_string()]
        );
    }

    #[test]
    fn two_fences_in_one_message_are_captured_separately() {
        let text = "one:\n```\naa\n```\ntwo:\n```\nbb\n```";
        assert_eq!(
            fences(text),
            vec![vec!["aa".to_string()], vec!["bb".to_string()]]
        );
        assert_eq!(
            texts(text),
            vec!["one:\n".to_string(), "\ntwo:\n".to_string()]
        );
    }

    #[test]
    fn an_empty_fence_captures_nothing() {
        assert!(fences("```\n```")[0].is_empty());
        assert!(fences("```\n\n```")[0].is_empty());
    }

    #[test]
    fn carriage_returns_are_stripped_from_captured_lines() {
        assert_eq!(
            fences("```\r\n..bb..\r\n```")[0],
            vec!["..bb..".to_string()]
        );
    }

    #[test]
    fn a_message_with_no_fence_is_one_piece_of_text() {
        assert_eq!(
            split_code_fences("Add a red brick."),
            vec![Segment::Text("Add a red brick.".to_string())]
        );
        assert!(split_code_fences("").is_empty());
    }

    #[test]
    fn a_fence_holding_multibyte_text_is_captured_intact() {
        // Byte offsets from the backtick scan must land on char boundaries.
        assert_eq!(
            fences("```\nkäytävä ×\n```")[0],
            vec!["käytävä ×".to_string()]
        );
    }

    fn shaped(lines: &[&str]) -> BlockShape {
        shape(&CodeBlock {
            lines: lines.iter().map(|l| (*l).to_string()).collect(),
        })
    }

    fn rows(layer: &[Vec<char>]) -> Vec<String> {
        layer.iter().map(|r| r.iter().collect()).collect()
    }

    #[test]
    fn a_legend_line_is_recognised_wherever_it_appears() {
        // Above, below or among the drawing — position carries no meaning.
        let s = shaped(&["b = blue 1x1 brick", "..bb..", "r = red 1x1 plate"]);
        assert_eq!(
            s.legend,
            vec![
                ('b', "blue 1x1 brick".to_string()),
                ('r', "red 1x1 plate".to_string())
            ]
        );
        assert_eq!(s.layers.len(), 1);
        assert_eq!(rows(&s.layers[0]), vec!["..bb..".to_string()]);
    }

    #[test]
    fn a_multi_character_legend_key_is_reported_not_drawn() {
        let s = shaped(&["bb = blue 1x1 brick", "..bb.."]);
        assert_eq!(s.bad_legend_key.as_deref(), Some("bb"));
        assert!(s.legend.is_empty());
    }

    #[test]
    fn a_row_containing_an_equals_sign_is_still_a_row() {
        // '=' is a perfectly good thing to draw with, so a row must not be
        // mistaken for a badly-keyed legend line.
        let s = shaped(&["= = red 1x1 tile", "..==.."]);
        assert_eq!(s.legend, vec![('=', "red 1x1 tile".to_string())]);
        assert!(s.bad_legend_key.is_none());
        assert_eq!(rows(&s.layers[0]), vec!["..==..".to_string()]);
    }

    #[test]
    fn blank_lines_separate_layers_and_empty_layers_are_dropped() {
        let s = shaped(&["b = blue 1x1 brick", "", "bb", "", "", "cc", ""]);
        assert_eq!(s.layers.len(), 2, "got {:?}", s.layers);
        assert_eq!(rows(&s.layers[0]), vec!["bb".to_string()]);
        assert_eq!(rows(&s.layers[1]), vec!["cc".to_string()]);
    }

    #[test]
    fn trailing_whitespace_is_removed_but_leading_is_kept() {
        // Trailing spaces are invisible and editors add them; leading ones are
        // empty cells the builder drew on purpose.
        let s = shaped(&["  bb  "]);
        assert_eq!(rows(&s.layers[0]), vec!["  bb".to_string()]);
    }

    #[test]
    fn a_block_of_only_legend_lines_has_no_layers() {
        let s = shaped(&["b = blue 1x1 brick"]);
        assert_eq!(s.legend.len(), 1);
        assert!(s.layers.is_empty());
    }

    #[test]
    fn a_legend_line_with_no_description_is_a_row() {
        assert_eq!(legend_line("b ="), LineShape::Row);
    }
}
