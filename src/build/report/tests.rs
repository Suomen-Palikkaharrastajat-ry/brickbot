use super::*;
use crate::build::dsl::lexer::Verb;
use crate::build::dsl::parser::Expectation;
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
        LowerError::Unsupported(Unsupported::That),
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
fn every_unsupported_gap_reads_as_a_sentence_in_both_locales() {
    let gaps = [
        Unsupported::That,
        Unsupported::Verb,
        Unsupported::MoveWithoutDirection,
        Unsupported::PaintWithoutColour,
        Unsupported::Replacement,
        Unsupported::Selection,
        Unsupported::Name,
        Unsupported::TooLarge {
            total: 900,
            limit: 400,
        },
        Unsupported::ExceedsMaxSize {
            x: 10,
            y: 10,
            z: 10,
        },
    ];

    for gap in gaps {
        for locale in ["en-US", "fi-FI"] {
            let text = describe_error(&LowerError::Unsupported(gap), locale);
            assert!(!text.is_empty(), "{gap:?} has no {locale} text");
            assert!(!text.contains("%{"), "unsubstituted: {text}");
            // A missing key falls through as the key itself.
            assert!(!text.contains("command.build."), "unresolved key: {text}");
        }
    }

    let too_large = describe_error(
        &LowerError::Unsupported(Unsupported::TooLarge {
            total: 900,
            limit: 400,
        }),
        "en-US",
    );
    assert!(too_large.contains("900") && too_large.contains("400"));

    let exceeds_max_size = describe_error(
        &LowerError::Unsupported(Unsupported::ExceedsMaxSize { x: 8, y: 12, z: 6 }),
        "en-US",
    );
    assert!(
        exceeds_max_size.contains('8')
            && exceeds_max_size.contains("12")
            && exceeds_max_size.contains('6')
    );
}

#[test]
fn a_size_that_does_not_exist_is_answered_with_ones_that_do() {
    let error = LowerError::NeedsPartLookup {
        query: "brick 1 x 7".to_string(),
        kind: PartKind::Brick,
    };
    for locale in ["en-US", "fi-FI"] {
        let text = describe_error(&error, locale);
        // Named the way they would be typed back, not as catalogue names.
        assert!(text.contains("1x6 brick"), "{locale}: {text}");
        assert!(text.contains("1x8 brick"), "{locale}: {text}");
        assert!(!text.contains("%{"), "unsubstituted: {text}");
    }
}

#[test]
fn a_part_number_with_no_size_falls_back_to_the_plain_message() {
    // Nothing to suggest sizes from, so it must not claim there is.
    let error = LowerError::NeedsPartLookup {
        query: "4073b".to_string(),
        kind: PartKind::Brick,
    };
    let text = describe_error(&error, "en-US");
    assert!(text.contains("4073b"), "got: {text}");
    assert!(!text.contains("Did you mean"), "got: {text}");
    assert!(!text.contains("%{"), "unsubstituted: {text}");
}

#[test]
fn an_unknown_drawing_symbol_says_which_ones_are_defined() {
    let error = LowerError::Grid(GridError::UnknownSymbol {
        symbol: 'z',
        col: 2,
        row: 3,
        layer: 1,
        known: "b, r".to_string(),
    });
    for locale in ["en-US", "fi-FI"] {
        let text = describe_error(&error, locale);
        assert!(text.contains('z'), "{locale}: {text}");
        assert!(text.contains("b, r"), "{locale}: {text}");
    }
}

#[test]
fn every_grid_problem_reads_as_a_sentence_in_both_locales() {
    let problems = [
        GridError::NoLegend,
        GridError::NoCells,
        GridError::ManyUnknownParts,
        GridError::LegendSymbolNotSingleChar {
            text: "bb".to_string(),
        },
        GridError::UnknownSymbol {
            symbol: 'z',
            col: 2,
            row: 3,
            layer: 1,
            known: "b, r".to_string(),
        },
        GridError::RaggedRows {
            layer: 1,
            row: 2,
            width: 5,
            expected: 6,
        },
        GridError::Unplaced {
            symbol: 't',
            col: 1,
            row: 1,
            layer: 2,
            count: 3,
        },
    ];

    for problem in problems {
        for locale in ["en-US", "fi-FI"] {
            let text = describe_error(&LowerError::Grid(problem.clone()), locale);
            assert!(!text.is_empty(), "{problem:?} has no {locale} text");
            assert!(!text.contains("%{"), "unsubstituted: {text}");
            // A missing key falls through as the key itself.
            assert!(!text.contains("command.build."), "unresolved key: {text}");
        }
    }
}

#[test]
fn every_expectation_is_named_in_both_locales() {
    let expectations = [
        Expectation::RevisionNumber,
        Expectation::SomethingToFind,
        Expectation::SomethingToShow,
        Expectation::Name,
        Expectation::SomethingToBuild,
        Expectation::PartToAdd,
        Expectation::ColourToPaintWith,
        Expectation::SomethingToReplace,
        Expectation::ReplacementPartOrColour,
        Expectation::SomethingToSelect,
        Expectation::GroupName,
        Expectation::RightAngle,
        Expectation::ViewName,
    ];

    for expected in expectations {
        for locale in ["en-US", "fi-FI"] {
            let error = ParseError {
                expected,
                found: Some("the".to_string()),
                sentence: "Add the".to_string(),
            };
            let text = describe_parse_error(&error, locale);
            assert!(!text.is_empty(), "{expected:?} has no {locale} text");
            assert!(!text.contains("%{"), "unsubstituted: {text}");
            assert!(!text.contains("command.build."), "unresolved key: {text}");
        }
    }
}

#[test]
fn a_parse_error_is_translated_rather_than_half_english() {
    let error = ParseError {
        expected: Expectation::PartToAdd,
        found: Some("the".to_string()),
        sentence: "Add the".to_string(),
    };
    let finnish = describe_parse_error(&error, "fi-FI");
    assert!(finnish.contains("lisättävää osaa"), "got: {finnish}");
    assert!(!finnish.contains("a part to add"), "got: {finnish}");
}

#[test]
fn an_error_quotes_the_offending_word() {
    let text = describe_error(&LowerError::UnknownName("gatehouse".to_string()), "en-US");
    assert!(text.contains("gatehouse"), "got: {text}");
}

#[test]
fn a_parse_error_names_what_was_expected() {
    let error = ParseError {
        expected: Expectation::PartToAdd,
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
        let text = help_text(None, locale);
        assert!(text.len() > 50, "{locale} help is suspiciously short");
        assert!(!text.contains("%{"));
        // The examples stay in English: the grammar is not translated.
        assert!(text.contains("Add a red 2x4 brick"));
    }
}

#[test]
fn every_documented_topic_has_its_own_help_in_both_locales() {
    let verbs = [
        Verb::Add,
        Verb::Move,
        Verb::Raise,
        Verb::Lower,
        Verb::Rotate,
        Verb::Remove,
        Verb::Replace,
        Verb::Paint,
        Verb::Build,
        Verb::Duplicate,
        Verb::Mirror,
        Verb::Select,
        Verb::Group,
        Verb::Name,
        Verb::Undo,
        Verb::Redo,
        Verb::Revert,
        Verb::Count,
        Verb::Find,
        Verb::Show,
        Verb::Export,
        Verb::Render,
        Verb::Help,
        Verb::Finish,
        Verb::Queries,
        Verb::Attach,
        Verb::Fill,
        Verb::Repeat,
    ];
    let documented: Vec<HelpTopic> = verbs
        .into_iter()
        .map(HelpTopic::Verb)
        .chain(HelpTopic::NAMED)
        .collect();

    let general = [help_text(None, "en-US"), help_text(None, "fi-FI")];

    for topic in documented {
        for locale in ["en-US", "fi-FI"] {
            let text = help_text(Some(topic), locale);
            assert!(!text.is_empty(), "{topic:?} has no {locale} help");
            assert!(!text.contains("%{"), "unsubstituted: {text}");
            assert!(
                !text.contains("command.build."),
                "unresolved key for {topic:?} in {locale}: {text}"
            );
        }
        assert_ne!(
            help_text(Some(topic), "en-US"),
            general[0],
            "{topic:?} just repeats the general help"
        );
    }
}

/// The general help advertises a list of topics. Every word on it has to be
/// a word `Help <topic>.` actually answers, or `Help.` is lying about what
/// the bot knows — the exact failure this whole help rework is about.
#[test]
fn every_topic_the_general_help_advertises_can_be_asked_for() {
    for locale in ["en-US", "fi-FI"] {
        let listed = t!("command.build.help_topics", locale = locale).to_string();
        let general = help_text(None, locale);

        let mut count = 0;
        for word in listed.split(',').map(str::trim) {
            let topic = HelpTopic::parse(word)
                .unwrap_or_else(|| panic!("{locale} advertises '{word}', which is not a topic"));
            let text = help_text(Some(topic), locale);
            assert_ne!(
                text, general,
                "'{word}' is advertised but falls back to the general help"
            );
            assert!(
                general.contains(word),
                "'{word}' is a topic but the {locale} general help does not name it"
            );
            count += 1;
        }
        assert!(count > 20, "{locale} advertises only {count} topics");
    }
}

/// `draw` lexes as a synonym of `render`, so before named topics existed
/// `Help draw.` answered "render the model again" — the opposite of the
/// question. Both spellings must reach drawings, and `render` must not.
#[test]
fn drawing_words_reach_the_drawings_topic_and_not_render() {
    let drawings = help_text(Some(HelpTopic::Drawings), "en-US");
    for word in ["draw", "drawing", "drawings", "grid", "grids"] {
        assert_eq!(
            help_text(HelpTopic::parse(word), "en-US"),
            drawings,
            "'{word}' should answer with the drawings topic"
        );
    }
    assert_ne!(
        help_text(HelpTopic::parse("render"), "en-US"),
        drawings,
        "'render' should still answer with the render topic"
    );
    assert_ne!(
        help_text(HelpTopic::parse("paint"), "en-US"),
        help_text(Some(HelpTopic::Parts), "en-US"),
        "'paint' should still answer with the paint topic"
    );
}

/// The drawings help is only useful if it carries a drawing that works.
/// A fenced legend and grid is also the longest thing the bot says, and
/// replies are sent unsplit, so it has to fit in one Discord message.
#[test]
fn the_drawings_help_carries_a_usable_example() {
    for locale in ["en-US", "fi-FI"] {
        let text = help_text(Some(HelpTopic::Drawings), locale);
        assert!(text.contains("Build from top at origin:"), "{locale}");
        assert!(text.contains("b = blue 1x1 brick"), "{locale}");
        // Exactly one pair. Discord pairs fences in the order it meets
        // them, so a stray ``` in the prose would open a code block at the
        // wrong word and swallow the example whole.
        assert_eq!(
            text.matches("```").count(),
            2,
            "{locale} drawings help must hold exactly one fence"
        );
        assert!(text.contains("Show top view."), "{locale}");
        assert!(
            text.len() < 2000,
            "{locale} drawings help is {} chars, over Discord's limit",
            text.len()
        );
    }
}
