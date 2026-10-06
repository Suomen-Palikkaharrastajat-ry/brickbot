//! Turning build outcomes into text for the thread.
//!
//! The only place `rust_i18n::t!` is called for build output, so every string
//! the feature shows a user lives in one file and in both locale files.

#![allow(clippy::doc_markdown)]

use super::dsl::lexer::HelpTopic;
use super::dsl::parser::ParseError;
use super::grid::GridError;
use super::legal::Adjustment;
use super::lower::{LowerError, Unsupported};
use super::model::Model;
use super::ops::{Effect, Op};
use super::queries::QueryAnswer;
use super::units::LDU_PLATE;
use crate::ldraw::colors;
use rust_i18n::t;

/// Describes what an operation did.
#[must_use]
pub fn describe_effect(op: &Op, effect: &Effect, locale: &str) -> String {
    if effect.no_op {
        return t!("command.build.nothing_happened", locale = locale).to_string();
    }

    let count = if effect.added.is_empty() {
        effect.affected.len()
    } else {
        effect.added.len()
    };

    let key = match op {
        Op::Add { .. } | Op::AddMany { .. } => "command.build.added",
        Op::Move { .. } => "command.build.moved",
        Op::Remove { .. } => "command.build.removed",
        Op::Paint { .. } => "command.build.painted",
        Op::Rotate { .. } => "command.build.rotated",
        Op::Replace { .. } => "command.build.replaced",
        Op::Select { .. } => "command.build.selected",
        Op::Group { .. } => "command.build.grouped",
        Op::Landmark { .. } => return t!("command.build.named_point", locale = locale).to_string(),
        Op::Rename { .. } => return t!("command.build.renamed", locale = locale).to_string(),
    };

    t!(key, locale = locale, count = count).to_string()
}

/// Describes a correction made to keep a placement legal, or nothing at all
/// when the command was already legal as written.
///
/// Silence is the common case and the important one: a builder who says
/// something buildable must not be told about geometry they never got wrong.
#[must_use]
pub fn describe_adjustment(adjustment: &Adjustment, locale: &str) -> Option<String> {
    if adjustment.is_none() {
        return None;
    }
    if adjustment.settled == 0 {
        return Some(
            t!(
                "command.build.adjusted_snapped",
                locale = locale,
                count = adjustment.snapped
            )
            .to_string(),
        );
    }
    // Positive is downward, because LDraw's Y axis points down.
    let key = if adjustment.settled > 0 {
        "command.build.adjusted_settled_down"
    } else {
        "command.build.adjusted_settled_up"
    };
    Some(
        t!(
            key,
            locale = locale,
            count = adjustment.settled_count,
            plates = adjustment.settled.abs() / LDU_PLATE
        )
        .to_string(),
    )
}

/// Names what is about to be placed, the way the builder said it.
#[must_use]
pub fn describe_part(
    part: &crate::build::PartRef,
    color: u16,
    quantity: u32,
    locale: &str,
) -> String {
    let named = colors::name_of(color).map_or_else(
        || t!("common.unknown_colour", locale = locale).to_string(),
        str::to_string,
    );
    t!(
        "command.build.propose_what",
        locale = locale,
        count = quantity,
        colour = named,
        part = part.name.clone()
    )
    .to_string()
}

/// Names a drawn block about to be placed.
#[must_use]
pub fn describe_drawing(parts: usize, locale: &str) -> String {
    t!(
        "command.build.propose_what_drawing",
        locale = locale,
        count = parts
    )
    .to_string()
}

/// Describes one proposed position, so the picture has a caption.
#[must_use]
pub fn describe_proposal(origin: crate::build::propose::Origin, locale: &str) -> String {
    origin.direction().map_or_else(
        || t!(origin.key(), locale = locale).to_string(),
        |direction| {
            t!(
                origin.key(),
                locale = locale,
                direction = t!(
                    format!("command.build.direction.{}", direction.as_str()),
                    locale = locale
                )
            )
            .to_string()
        },
    )
}

/// Describes why a well-formed sentence could not be carried out.
#[must_use]
pub fn describe_error(error: &LowerError, locale: &str) -> String {
    match error {
        LowerError::UnresolvedReference(what) => t!(
            "command.build.error_unresolved",
            locale = locale,
            what = what
        )
        .to_string(),
        LowerError::UnknownName(name) => t!(
            "command.build.error_unknown_name",
            locale = locale,
            name = name
        )
        .to_string(),
        LowerError::NothingToActOn => {
            t!("command.build.error_nothing", locale = locale).to_string()
        }
        LowerError::UnknownStructure(name) => t!(
            "command.build.error_unknown_structure",
            locale = locale,
            name = name
        )
        .to_string(),
        // A size the catalogue does not have is worth answering with the ones
        // it does: `1x7 brick` is not an element, and naming 1x6 and 1x8 is
        // more use than sending the builder to a search that cannot succeed.
        LowerError::NeedsPartLookup { query, kind } => {
            let alternatives = crate::build::parts::footprint_from_name(query)
                .map(|(w, d)| crate::build::parts::suggest(*kind, w, d, 2))
                .unwrap_or_default();
            if alternatives.is_empty() {
                t!(
                    "command.build.error_needs_lookup",
                    locale = locale,
                    query = query
                )
                .to_string()
            } else {
                t!(
                    "command.build.error_needs_lookup_did_you_mean",
                    locale = locale,
                    query = query,
                    suggestions = join_parts(&alternatives)
                )
                .to_string()
            }
        }
        // Each gap is its own sentence rather than a fragment slotted into one
        // template: "a paint with no colour" does not decline into Finnish, and
        // asking the builder for the missing piece is more useful anyway.
        LowerError::Unsupported(what) => match what {
            Unsupported::TooLarge { total, limit } => {
                t!(what.key(), locale = locale, total = total, limit = limit).to_string()
            }
            Unsupported::ExceedsMaxSize { x, y, z } => {
                t!(what.key(), locale = locale, x = x, y = y, z = z).to_string()
            }
            _ => t!(what.key(), locale = locale).to_string(),
        },
        LowerError::Grid(problem) => describe_grid_error(problem, locale),
    }
}

/// The body of a private drawing preview.
///
/// The drawing itself, then whatever the interpreter said — the same replies
/// the thread would have seen, so nothing is hidden by previewing. Capped to
/// one Discord message, because a preview that fails to post is worse than a
/// shortened one.
#[must_use]
pub fn draft_preview(preview: &crate::build::draft::Preview, locale: &str) -> String {
    use crate::build::textview;
    use crate::discord_limits::MAX_MESSAGE_CONTENT;

    let mut out = if preview.failed() {
        t!("command.build.draft_problem", locale = locale).to_string()
    } else {
        t!(
            "command.build.draft_ready",
            locale = locale,
            parts = preview.model.len()
        )
        .to_string()
    };

    // The drawing as it would stand, in the syntax it was typed in.
    if let Some(view) = textview::render(&preview.model, preview.view, None) {
        let mut view = view;
        let budget = MAX_MESSAGE_CONTENT.saturating_sub(out.len() + 400);
        textview::fit(&mut view, budget);
        let block = textview::to_block(&view);
        if block.len() <= budget {
            out.push('\n');
            out.push_str(&block);
        }
    }

    for reply in &preview.outcome.replies {
        out.push('\n');
        out.push_str(reply);
    }

    crate::discord_limits::truncate(&out, MAX_MESSAGE_CONTENT)
}

/// Lists parts the way a builder would type them, not as catalogue names.
///
/// A suggestion has to be something that can be pasted straight back into a
/// sentence or a legend, so it reads `1x6 brick` rather than `Brick 1 x 6`.
fn join_parts(parts: &[crate::build::PartRef]) -> String {
    parts
        .iter()
        .map(|part| {
            format!(
                "{}x{} {}",
                part.footprint.0,
                part.footprint.1,
                part.kind.as_str()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Describes why a drawing could not be read.
///
/// Each variant supplies its own placeholders, the way `Unsupported::TooLarge`
/// does above. Positions are already 1-based when they reach here.
fn describe_grid_error(problem: &GridError, locale: &str) -> String {
    let key = problem.key();
    match problem {
        GridError::NoLegend | GridError::NoCells | GridError::ManyUnknownParts => {
            t!(key, locale = locale).to_string()
        }
        GridError::LegendSymbolNotSingleChar { text } => {
            t!(key, locale = locale, text = text).to_string()
        }
        GridError::UnknownSymbol {
            symbol,
            col,
            row,
            layer,
            known,
        } => t!(
            key,
            locale = locale,
            symbol = symbol,
            col = col,
            row = row,
            layer = layer,
            known = known
        )
        .to_string(),
        GridError::RaggedRows {
            layer,
            row,
            width,
            expected,
        } => t!(
            key,
            locale = locale,
            layer = layer,
            row = row,
            width = width,
            expected = expected
        )
        .to_string(),
        GridError::Unplaced {
            symbol,
            col,
            row,
            layer,
            count,
        } => t!(
            key,
            locale = locale,
            symbol = symbol,
            col = col,
            row = row,
            layer = layer,
            count = count
        )
        .to_string(),
    }
}

/// Describes a sentence that opened with a verb but did not parse.
#[must_use]
pub fn describe_parse_error(error: &ParseError, locale: &str) -> String {
    error.found.as_ref().map_or_else(
        || {
            t!(
                "command.build.error_parse_general",
                locale = locale,
                expected = t!(error.expected.key(), locale = locale)
            )
            .to_string()
        },
        |found| {
            t!(
                "command.build.error_parse",
                locale = locale,
                found = found,
                expected = t!(error.expected.key(), locale = locale)
            )
            .to_string()
        },
    )
}

/// Describes the answer to a question.
#[must_use]
pub fn describe_answer(answer: &QueryAnswer, model: &Model, locale: &str) -> String {
    match answer {
        QueryAnswer::Height {
            ldu,
            bricks,
            plates,
        } => t!(
            "command.build.answer_height",
            locale = locale,
            name = &model.name,
            bricks = bricks,
            plates = plates,
            ldu = ldu
        )
        .to_string(),

        QueryAnswer::Count(count) => {
            t!("command.build.answer_count", locale = locale, count = count).to_string()
        }

        QueryAnswer::Collisions(pairs) => t!(
            "command.build.answer_collisions",
            locale = locale,
            count = pairs.len()
        )
        .to_string(),

        QueryAnswer::Unsupported(ids) => t!(
            "command.build.answer_unsupported",
            locale = locale,
            count = ids.len()
        )
        .to_string(),

        QueryAnswer::Position(pos) => t!(
            "command.build.answer_position",
            locale = locale,
            x = pos.x,
            y = pos.y,
            z = pos.z
        )
        .to_string(),

        // The log stores user ids, so render one as a mention: Discord turns
        // it into the person's name. The caller suppresses allowed_mentions,
        // so it reads as a name without pinging anyone.
        QueryAnswer::Author(author) => t!(
            "command.build.answer_author",
            locale = locale,
            author = format!("<@{author}>")
        )
        .to_string(),

        QueryAnswer::Changed(Some(what)) => {
            t!("command.build.answer_changed", locale = locale, what = what).to_string()
        }
        QueryAnswer::Changed(None) => {
            t!("command.build.answer_nothing_changed", locale = locale).to_string()
        }

        QueryAnswer::Inventory(parts) => {
            let list = parts
                .iter()
                .map(|(part, color, quantity)| {
                    let color_name = colors::name_of(*color).map_or_else(
                        || t!("common.unknown_colour", locale = locale).to_string(),
                        str::to_string,
                    );
                    format!("- {quantity}x {} ({color_name})", part.name)
                })
                .collect::<Vec<_>>()
                .join("\n");
            t!(
                "command.build.answer_inventory",
                locale = locale,
                list = list
            )
            .to_string()
        }

        QueryAnswer::View(drawing) => describe_view(drawing, locale),

        QueryAnswer::NothingToReport => {
            t!("command.build.answer_nothing", locale = locale).to_string()
        }
    }
}

/// Wraps a drawing in a localized sentence, trimmed to fit one Discord message.
///
/// The only place that knows both how long the wrapper is and how much room
/// Discord leaves, which is why the size guard lives here rather than in
/// [`crate::build::textview`]. Layers go whole: half a layer is not something
/// anyone can paste back.
fn describe_view(drawing: &crate::build::textview::TextView, locale: &str) -> String {
    use crate::build::textview;
    use crate::discord_limits::MAX_MESSAGE_CONTENT;

    let view = drawing.view.as_str();
    let wrapper = t!(
        "command.build.answer_view",
        locale = locale,
        name = &drawing.model_name,
        view = view,
        block = ""
    )
    .to_string();

    // Leave room for the wrapper and for the truncation note that may follow.
    let budget = MAX_MESSAGE_CONTENT.saturating_sub(wrapper.len() + 200);

    let mut drawing = drawing.clone();
    let dropped = textview::fit(&mut drawing, budget);
    let block = textview::to_block(&drawing);

    if block.len() > budget {
        // Not even one layer fits, so there is nothing useful to show.
        return t!(
            "command.build.answer_view_too_big",
            locale = locale,
            view = view
        )
        .to_string();
    }

    let mut out = t!(
        "command.build.answer_view",
        locale = locale,
        name = &drawing.model_name,
        view = view,
        block = block
    )
    .to_string();

    if dropped > 0 {
        out.push_str(&t!(
            "command.build.answer_view_truncated",
            locale = locale,
            count = dropped,
            view = view
        ));
    }
    out
}

/// The caption posted alongside a render.
#[must_use]
pub fn render_caption(model: &Model, revision: usize, locale: &str) -> String {
    t!(
        "command.build.render_caption",
        locale = locale,
        name = &model.name,
        parts = model.len(),
        revision = revision
    )
    .to_string()
}

/// The help text: the general overview, or one topic's syntax and examples.
///
/// A word [`HelpTopic::help_key`] doesn't cover falls back to the general
/// text. Asking for help wrong is still asking for help, so it must never come
/// back as an error.
///
/// The general text lists what can be asked about from
/// `command.build.help_topics`, the one place the topic list is written down.
/// Interpolating it rather than repeating it in prose is what keeps `Help.`
/// honest about what `Help <topic>.` will actually answer, in every locale.
#[must_use]
pub fn help_text(topic: Option<HelpTopic>, locale: &str) -> String {
    topic.and_then(HelpTopic::help_key).map_or_else(
        || {
            t!(
                "command.build.help",
                locale = locale,
                topics = t!("command.build.help_topics", locale = locale)
            )
            .to_string()
        },
        |key| t!(key, locale = locale).to_string(),
    )
}

#[cfg(test)]
mod tests;
