//! Drafting a drawing privately, before it lands in the thread.
//!
//! A build thread is public and collaborative, so every attempt at a drawing
//! is seen by everyone. This module backs a private form: the builder fills in
//! a view, a legend, a grid and an optional location, sees a preview only they
//! can see, and applies it when it looks right.
//!
//! Pure — no serenity, no sqlx. The Discord shell in [`super::session`] owns
//! the form and the buttons; everything here is text and interpretation.
//!
//! Two properties do the work:
//!
//! * [`assemble`] produces exactly the message a builder would have typed, so
//!   the preview and the apply interpret the same string and cannot diverge.
//! * That string has the same shape [`super::textview::to_block`] emits, so
//!   [`split_fields`] can fill the form from what is *already built*. Editing
//!   the current model is the easiest way to pick parts that exist.

#![allow(clippy::doc_markdown)]

use super::dispatch::{self, Builders, Outcome};
use super::ops::History;
use super::textview::TextView;

/// The four inputs of the drawing form.
///
/// Held as typed, so the form can be reopened exactly as it was left even when
/// what it holds does not parse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftFields {
    /// `top`, `north`, … — validated when the drawing is interpreted.
    pub view: String,
    /// One `b = blue 1x1 brick` per line.
    pub legend: String,
    /// The drawing. Blank lines separate layers.
    pub grid: String,
    /// Any ordinary location phrase, or empty.
    pub location: String,
}

impl DraftFields {
    /// True when there is nothing worth interpreting yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.view.trim().is_empty() && self.legend.trim().is_empty() && self.grid.trim().is_empty()
    }
}

/// Assembles the form into the BrickTalk message it stands for.
///
/// The builder never types the fence, so it can never be malformed — which is
/// most of what makes a drawing fiddly to get right by hand.
#[must_use]
pub fn assemble(fields: &DraftFields) -> String {
    let view = {
        let trimmed = fields.view.trim();
        if trimmed.is_empty() { "top" } else { trimmed }
    };

    let mut header = format!("Build from {view}");
    let location = fields.location.trim();
    if !location.is_empty() {
        header.push(' ');
        header.push_str(location);
    }
    header.push(':');

    let legend = fields.legend.trim_end();
    let grid = fields.grid.trim_end();

    let mut out = format!("{header}\n```\n");
    if !legend.is_empty() {
        out.push_str(legend);
        // The blank line is what separates a legend from the drawing; without
        // it the first row would be read as another layer boundary.
        out.push_str("\n\n");
    }
    out.push_str(grid);
    out.push_str("\n```");
    out
}

/// Fills the form from a drawing the bot produced.
///
/// The inverse of [`assemble`] for anything [`super::textview`] emits, which
/// is what lets the form open showing the current model rather than a blank
/// page.
#[must_use]
pub fn split_fields(view: &TextView) -> DraftFields {
    DraftFields {
        view: view.view.as_str().to_string(),
        legend: view
            .legend
            .iter()
            .map(|(symbol, text)| format!("{symbol} = {text}"))
            .collect::<Vec<_>>()
            .join("\n"),
        grid: view
            .layers
            .iter()
            .map(|rows| rows.join("\n"))
            .collect::<Vec<_>>()
            .join("\n\n"),
        // `to_block` always anchors at the origin, and so does a drawing
        // rebuilt from it.
        location: "at origin".to_string(),
    }
}

/// Symbols to fall back on once the mnemonic ones are taken.
///
/// Deliberately excludes `.` and space, which mean "nothing here", and `=`,
/// which would make the legend line ambiguous.
const SYMBOL_POOL: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

/// The keys a legend already uses.
fn used_symbols(legend: &str) -> Vec<char> {
    legend
        .lines()
        .filter_map(|line| match super::dsl::block::legend_line(line) {
            super::dsl::block::LineShape::Legend { symbol, .. } => Some(symbol),
            _ => None,
        })
        .collect()
}

/// Picks a legend key that is not taken yet.
///
/// Prefers the colour's own initial, the way [`super::textview`] assigns
/// symbols when it draws a model — so blue tends to come out as `b` and the
/// legend reads as something a person would have written.
#[must_use]
pub fn next_symbol(legend: &str, colour: Option<&str>) -> char {
    let taken = used_symbols(legend);
    colour
        .and_then(|name| name.chars().find(char::is_ascii_lowercase))
        .into_iter()
        .chain(SYMBOL_POOL.chars())
        .find(|candidate| !taken.contains(candidate))
        // Every symbol taken is not a real drawing, but it must not panic.
        .unwrap_or('?')
}

/// Adds a legend line, in the order the part grammar reads best.
///
/// `b = blue 1x1 brick`. With no colour the line is still valid — the drawing
/// simply takes the default — which is what lets a part be picked before a
/// colour has been chosen.
pub fn append_legend_line(
    fields: &mut DraftFields,
    symbol: char,
    colour: Option<&str>,
    part: &str,
) {
    use std::fmt::Write;

    let part = part.trim();
    let described = colour
        .map(str::trim)
        .filter(|colour| !colour.is_empty())
        .map_or_else(|| part.to_string(), |colour| format!("{colour} {part}"));

    if !fields.legend.is_empty() && !fields.legend.ends_with('\n') {
        fields.legend.push('\n');
    }
    let _ = write!(fields.legend, "{symbol} = {described}");
}

/// A drawing interpreted against the session, without anything being written.
#[derive(Debug, Clone)]
pub struct Preview {
    /// What the builder typed, assembled.
    pub text: String,
    /// Everything the interpreter decided — the operations, the replies, and
    /// any problems. Never persisted.
    pub outcome: Outcome,
    /// The model as it would be once applied.
    pub model: super::model::Model,
    /// The view the draft was drawn from, so the preview is shown the same way
    /// round as it was typed.
    pub view: super::units::GridView,
    /// The revision this was interpreted against, which [`is_stale`] compares
    /// on apply.
    pub based_on: usize,
}

impl Preview {
    /// True when the drawing could not be carried out, so there is nothing to
    /// apply.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.outcome.ops.is_empty()
    }
}

/// Interprets a draft without persisting anything.
///
/// The whole private loop in one function: clone the history, run the ordinary
/// interpreter over the assembled text, and keep the result to one side. Going
/// back through [`dispatch::process_message`] rather than a preview-specific
/// path is what guarantees the preview and the eventual apply agree.
#[must_use]
pub fn preview(
    history: &History,
    fields: &DraftFields,
    locale: &str,
    builders: &Builders,
    author: &str,
) -> Preview {
    let text = assemble(fields);
    let outcome = dispatch::process_message(history, &text, locale, builders, author);

    // The model as it would stand, built the same way the session builds it.
    let mut projected = history.clone();
    for op in &outcome.ops {
        projected.push(op.clone(), author);
    }

    Preview {
        text,
        outcome,
        model: projected.materialize(),
        // An unreadable view name is reported by the interpreter; showing the
        // result from the top is the least surprising fallback.
        view: super::units::GridView::parse(fields.view.trim())
            .unwrap_or(super::units::GridView::Top),
        based_on: history.revision(),
    }
}

/// Whether the session has moved since a preview was taken.
///
/// Applying is refused when it has: what the builder saw must be exactly what
/// lands, and a placement id from a stale preview may no longer resolve.
#[must_use]
pub const fn is_stale(preview: &Preview, current: &History) -> bool {
    preview.based_on != current.revision()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::textview;
    use crate::build::units::GridView;

    fn fields() -> DraftFields {
        DraftFields {
            view: "top".to_string(),
            legend: "b = blue 1x1 brick\nr = red 1x1 brick".to_string(),
            grid: ".bb.\nrbbr".to_string(),
            location: "at origin".to_string(),
        }
    }

    fn builders() -> Builders {
        Builders::new([("Alice".to_string(), "111".to_string())])
    }

    fn empty() -> History {
        History::new("Castle")
    }

    #[test]
    fn the_form_assembles_into_the_sentence_it_stands_for() {
        assert_eq!(
            assemble(&fields()),
            "Build from top at origin:\n\
             ```\n\
             b = blue 1x1 brick\n\
             r = red 1x1 brick\n\
             \n\
             .bb.\n\
             rbbr\n\
             ```"
        );
    }

    #[test]
    fn an_omitted_location_and_view_fall_back_rather_than_breaking() {
        let assembled = assemble(&DraftFields {
            view: String::new(),
            legend: "b = blue 1x1 brick".to_string(),
            grid: "bb".to_string(),
            location: String::new(),
        });
        assert!(
            assembled.starts_with("Build from top:\n"),
            "got: {assembled}"
        );
    }

    #[test]
    fn an_assembled_draft_builds_the_model() {
        // The point of the whole module: what the form assembles is ordinary
        // BrickTalk, so the ordinary interpreter handles it.
        let preview = preview(&empty(), &fields(), "en-US", &builders(), "111");
        assert!(!preview.failed(), "got: {:?}", preview.outcome);
        assert_eq!(preview.model.len(), 6);
        assert_eq!(preview.model.count_matching(|p| p.color == 1), 4);
        assert_eq!(preview.model.count_matching(|p| p.color == 4), 2);
    }

    #[test]
    fn a_preview_never_touches_the_history_it_was_given() {
        let history = empty();
        let before = history.clone();
        let _ = preview(&history, &fields(), "en-US", &builders(), "111");
        assert_eq!(history, before, "preview must not mutate the session");
    }

    #[test]
    fn a_drawing_that_cannot_be_read_previews_as_a_failure_with_its_reason() {
        let broken = DraftFields {
            grid: ".bz.".to_string(),
            ..fields()
        };
        let preview = preview(&empty(), &broken, "en-US", &builders(), "111");
        assert!(preview.failed());
        assert_eq!(preview.outcome.replies.len(), 1);
        assert!(
            preview.outcome.replies[0].contains('z'),
            "got: {:?}",
            preview.outcome.replies
        );
    }

    #[test]
    fn a_preview_is_stale_once_anything_else_lands() {
        let history = empty();
        let preview = preview(&history, &fields(), "en-US", &builders(), "111");
        assert!(!is_stale(&preview, &history));

        // Someone else builds meanwhile.
        let mut moved = history;
        for op in &preview.outcome.ops {
            moved.push(op.clone(), "222");
        }
        assert!(is_stale(&preview, &moved));
    }

    #[test]
    fn the_form_can_be_filled_from_what_is_already_built() {
        // Build something, draw it back, and the form holds it — which is the
        // easiest way to pick parts that certainly exist.
        let preview = preview(&empty(), &fields(), "en-US", &builders(), "111");
        let drawn = textview::render(&preview.model, GridView::Top, None).unwrap();
        let refilled = split_fields(&drawn);

        assert_eq!(refilled.view, "top");
        assert!(
            refilled.legend.contains("blue 1x1 brick"),
            "got: {}",
            refilled.legend
        );
        assert!(!refilled.grid.is_empty());
    }

    #[test]
    fn a_form_filled_from_a_drawing_rebuilds_the_same_model() {
        // The round trip that makes the prefill trustworthy.
        let first = preview(&empty(), &fields(), "en-US", &builders(), "111");
        let drawn = textview::render(&first.model, GridView::Top, None).unwrap();
        let second = preview(&empty(), &split_fields(&drawn), "en-US", &builders(), "111");

        let describe = |model: &crate::build::Model| {
            let mut rows: Vec<_> = model
                .placements
                .iter()
                .map(|p| (p.part.ldraw_id.clone(), p.color, p.pos))
                .collect();
            rows.sort();
            rows
        };
        assert_eq!(describe(&second.model), describe(&first.model));
    }

    #[test]
    fn a_new_symbol_prefers_the_colours_own_initial() {
        // So the legend reads the way a person would have written it.
        assert_eq!(next_symbol("", Some("blue")), 'b');
        assert_eq!(next_symbol("", Some("red")), 'r');
        assert_eq!(next_symbol("", Some("dark tan")), 'd');
    }

    #[test]
    fn a_new_symbol_avoids_the_ones_already_in_the_legend() {
        let legend = "b = blue 1x1 brick\nr = red 1x1 brick";
        // 'b' is taken, so blue falls through to the pool.
        let picked = next_symbol(legend, Some("blue"));
        assert_ne!(picked, 'b');
        assert_ne!(picked, 'r');
        assert!(picked.is_ascii_alphanumeric());
    }

    #[test]
    fn a_new_symbol_works_with_no_colour_at_all() {
        assert!(next_symbol("", None).is_ascii_alphanumeric());
        assert_ne!(next_symbol("a = red 1x1 brick", None), 'a');
    }

    #[test]
    fn a_legend_using_every_symbol_still_answers_rather_than_panicking() {
        let legend: String = SYMBOL_POOL
            .chars()
            .map(|c| format!("{c} = red 1x1 brick"))
            .collect::<Vec<_>>()
            .join("\n");
        // Not a real drawing, but it must not bring the handler down.
        assert_eq!(next_symbol(&legend, Some("blue")), '?');
    }

    #[test]
    fn a_line_is_appended_in_the_order_the_grammar_reads_best() {
        let mut fields = DraftFields::default();
        append_legend_line(&mut fields, 'b', Some("blue"), "1x1 brick");
        assert_eq!(fields.legend, "b = blue 1x1 brick");

        append_legend_line(&mut fields, 'r', Some("red"), "1x1 plate");
        assert_eq!(fields.legend, "b = blue 1x1 brick\nr = red 1x1 plate");
    }

    #[test]
    fn a_line_with_no_colour_is_still_valid() {
        // Which is what lets a part be picked before a colour has been.
        let mut fields = DraftFields::default();
        append_legend_line(&mut fields, 'b', None, "1x1 brick");
        assert_eq!(fields.legend, "b = 1x1 brick");
        // And blank is treated as absent, not as an empty word.
        let mut fields = DraftFields::default();
        append_legend_line(&mut fields, 'b', Some("  "), "1x1 brick");
        assert_eq!(fields.legend, "b = 1x1 brick");
    }

    #[test]
    fn appending_does_not_double_up_a_trailing_newline() {
        let mut fields = DraftFields {
            legend: "b = blue 1x1 brick\n".to_string(),
            ..DraftFields::default()
        };
        append_legend_line(&mut fields, 'r', Some("red"), "1x1 plate");
        assert_eq!(fields.legend, "b = blue 1x1 brick\nr = red 1x1 plate");
    }

    #[test]
    fn an_appended_line_is_read_back_as_a_legend_entry() {
        // The property that matters: what we write must parse as what we meant.
        let mut fields = fields();
        let symbol = next_symbol(&fields.legend, Some("tan"));
        append_legend_line(&mut fields, symbol, Some("tan"), "1x1 tile");

        let preview = preview(&empty(), &fields, "en-US", &builders(), "111");
        // The drawing still uses only b and r, so adding a legend entry alone
        // changes nothing — but it must not break the parse either.
        assert!(!preview.failed(), "got: {:?}", preview.outcome);
        assert!(fields.legend.contains(&format!("{symbol} = tan 1x1 tile")));
    }

    #[test]
    fn an_empty_form_is_recognised_before_it_is_interpreted() {
        assert!(DraftFields::default().is_empty());
        assert!(!fields().is_empty());
    }
}
