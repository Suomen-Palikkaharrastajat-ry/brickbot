//! Turning build outcomes into text for the thread.
//!
//! The only place `rust_i18n::t!` is called for build output, so every string
//! the feature shows a user lives in one file and in both locale files.

#![allow(clippy::doc_markdown)]

use super::dsl::parser::ParseError;
use super::lower::LowerError;
use super::model::Model;
use super::ops::{Effect, Op};
use super::queries::QueryAnswer;
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
        LowerError::NeedsPartLookup { query, .. } => t!(
            "command.build.error_needs_lookup",
            locale = locale,
            query = query
        )
        .to_string(),
        LowerError::Unsupported(what) => t!(
            "command.build.error_unsupported",
            locale = locale,
            what = what
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
                expected = &error.expected
            )
            .to_string()
        },
        |found| {
            t!(
                "command.build.error_parse",
                locale = locale,
                found = found,
                expected = &error.expected
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
                    let color_name = colors::name_of(*color).unwrap_or("unknown");
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

        QueryAnswer::NothingToReport => {
            t!("command.build.answer_nothing", locale = locale).to_string()
        }
    }
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

/// The help text.
#[must_use]
pub fn help_text(locale: &str) -> String {
    t!("command.build.help", locale = locale).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::model::{PartKind, PartRef, PlacementId};
    use crate::build::units::IVec3;

    fn effect_adding(count: usize) -> Effect {
        Effect {
            added: (0..count)
                .map(|i| PlacementId::new(0, u32::try_from(i).unwrap()))
                .collect(),
            affected: Vec::new(),
            no_op: count == 0,
        }
    }

    fn add_op() -> Op {
        Op::Add {
            part: PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
            color: 4,
            pos: IVec3::ZERO,
            orientation: crate::build::Orientation::DEFAULT,
            repeat: None,
            group: None,
        }
    }

    #[test]
    fn describes_an_addition_in_both_locales() {
        let effect = effect_adding(3);
        let english = describe_effect(&add_op(), &effect, "en-US");
        let finnish = describe_effect(&add_op(), &effect, "fi-FI");

        assert!(english.contains('3'), "got: {english}");
        assert!(finnish.contains('3'), "got: {finnish}");
        assert_ne!(english, finnish, "the locales must actually differ");
        assert!(english.to_lowercase().contains("added"));
        assert!(finnish.to_lowercase().contains("lisätty"));
    }

    #[test]
    fn a_no_op_says_nothing_changed() {
        let effect = Effect {
            no_op: true,
            ..Effect::default()
        };
        for locale in ["en-US", "fi-FI"] {
            let text = describe_effect(&add_op(), &effect, locale);
            assert!(!text.is_empty());
            assert!(!text.contains("%{"), "unsubstituted placeholder in {text}");
        }
    }

    #[test]
    fn every_operation_has_a_description_in_both_locales() {
        let ops = [
            add_op(),
            Op::AddMany {
                placements: Vec::new(),
                group: None,
            },
            Op::Move {
                targets: vec![PlacementId::new(0, 0)],
                offset: IVec3::ZERO,
            },
            Op::Remove {
                targets: vec![PlacementId::new(0, 0)],
            },
            Op::Paint {
                targets: vec![PlacementId::new(0, 0)],
                color: 4,
            },
            Op::Rotate {
                targets: vec![PlacementId::new(0, 0)],
                turns: 1,
                flip: false,
            },
            Op::Replace {
                targets: vec![PlacementId::new(0, 0)],
                part: PartRef::new("3003", "Brick 2 x 2", (2, 2), PartKind::Brick),
            },
            Op::Select {
                predicate: crate::build::SelectPredicate::default(),
            },
            Op::Group {
                name: "wall".to_string(),
                targets: vec![PlacementId::new(0, 0)],
            },
            Op::Landmark {
                name: "gate".to_string(),
                pos: IVec3::ZERO,
            },
            Op::Rename {
                name: "Castle".to_string(),
            },
        ];
        let effect = Effect {
            affected: vec![PlacementId::new(0, 0)],
            ..Effect::default()
        };

        for op in &ops {
            for locale in ["en-US", "fi-FI"] {
                let text = describe_effect(op, &effect, locale);
                assert!(!text.is_empty(), "{op:?} has no {locale} description");
                assert!(!text.contains("%{"), "unsubstituted placeholder: {text}");
            }
        }
    }

    #[test]
    fn describes_every_error_in_both_locales() {
        let errors = [
            LowerError::UnresolvedReference("that".to_string()),
            LowerError::UnknownName("roof".to_string()),
            LowerError::NothingToActOn,
            LowerError::UnknownStructure("gazebo".to_string()),
            LowerError::NeedsPartLookup {
                query: "3x12 brick".to_string(),
                kind: PartKind::Brick,
            },
            LowerError::Unsupported("that".to_string()),
        ];

        for error in &errors {
            let english = describe_error(error, "en-US");
            let finnish = describe_error(error, "fi-FI");
            assert!(!english.is_empty());
            assert!(!finnish.is_empty());
            assert_ne!(english, finnish, "{error:?} reads the same in both locales");
            assert!(!english.contains("%{"), "unsubstituted: {english}");
            assert!(!finnish.contains("%{"), "unsubstituted: {finnish}");
        }
    }

    #[test]
    fn an_error_quotes_the_offending_word() {
        let text = describe_error(&LowerError::UnknownName("gatehouse".to_string()), "en-US");
        assert!(text.contains("gatehouse"), "got: {text}");
    }

    #[test]
    fn a_parse_error_names_what_was_expected() {
        let error = ParseError {
            expected: "a part to add".to_string(),
            found: Some("the".to_string()),
            sentence: "Add the".to_string(),
        };
        let text = describe_parse_error(&error, "en-US");
        assert!(text.contains("the"), "got: {text}");
        assert!(text.contains("a part to add"), "got: {text}");

        let no_token = ParseError {
            found: None,
            ..error
        };
        let text = describe_parse_error(&no_token, "en-US");
        assert!(text.contains("a part to add"));
        assert!(!text.contains("%{"));
    }

    #[test]
    fn describes_every_answer_in_both_locales() {
        let model = Model::new("Castle");
        let answers = [
            QueryAnswer::Height {
                ldu: 48,
                bricks: 2,
                plates: 6,
            },
            QueryAnswer::Count(7),
            QueryAnswer::Collisions(vec![(PlacementId::new(0, 0), PlacementId::new(1, 0))]),
            QueryAnswer::Unsupported(vec![PlacementId::new(1, 0)]),
            QueryAnswer::Position(IVec3::new(20, -24, 40)),
            QueryAnswer::Author("alice".to_string()),
            QueryAnswer::Changed(Some("added Brick 2 x 4".to_string())),
            QueryAnswer::Changed(None),
            QueryAnswer::Inventory(vec![(
                PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
                4,
                3,
            )]),
            QueryAnswer::NothingToReport,
        ];

        for answer in &answers {
            for locale in ["en-US", "fi-FI"] {
                let text = describe_answer(answer, &model, locale);
                assert!(!text.is_empty(), "{answer:?} has no {locale} text");
                assert!(!text.contains("%{"), "unsubstituted: {text}");
            }
        }
    }

    #[test]
    fn a_height_answer_names_the_model_and_its_size() {
        let model = Model::new("Castle");
        let text = describe_answer(
            &QueryAnswer::Height {
                ldu: 48,
                bricks: 2,
                plates: 6,
            },
            &model,
            "en-US",
        );
        assert!(text.contains("Castle"), "got: {text}");
        assert!(text.contains('2') && text.contains('6'), "got: {text}");
    }

    #[test]
    fn an_inventory_lists_quantities_colours_and_names() {
        let model = Model::new("t");
        let text = describe_answer(
            &QueryAnswer::Inventory(vec![(
                PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
                4,
                3,
            )]),
            &model,
            "en-US",
        );
        assert!(text.contains("3x"), "got: {text}");
        assert!(text.contains("Brick 2 x 4"), "got: {text}");
        assert!(text.contains("red"), "got: {text}");
    }

    #[test]
    fn the_render_caption_reports_the_model_state() {
        let mut model = Model::new("Castle");
        model.placements = vec![];
        let text = render_caption(&model, 7, "en-US");
        assert!(text.contains("Castle"));
        assert!(text.contains('7'));
    }

    #[test]
    fn help_text_exists_in_both_locales() {
        for locale in ["en-US", "fi-FI"] {
            let text = help_text(locale);
            assert!(text.len() > 50, "{locale} help is suspiciously short");
            assert!(!text.contains("%{"));
            // The examples stay in English: the grammar is not translated.
            assert!(text.contains("Add a red 2x4 brick"));
        }
    }
}
