use super::*;
use crate::dsl::lexer::tokenize_sentence;

fn parse(text: &str) -> SentenceOutcome {
    parse_sentence(&tokenize_sentence(text))
}

fn proposal(text: &str) -> (PartSpec, Option<Arrangement>) {
    match parse(text) {
        SentenceOutcome::Statement(Statement::Propose { spec, arrangement }) => (spec, arrangement),
        other => panic!("expected a proposal from {text:?}, got {other:?}"),
    }
}

#[test]
fn a_bare_part_phrase_is_a_proposal_not_chat() {
    let (spec, arrangement) = proposal("2x2 brick");
    assert_eq!(spec.size, Some((2, 2)));
    assert_eq!(spec.kind, Some(PartKind::Brick));
    assert_eq!(spec.quantity, 1);
    assert_eq!(arrangement, None);
}

#[test]
fn a_part_phrase_carries_its_count_and_colour() {
    let (spec, _) = proposal("2 red 2x4 bricks");
    assert_eq!(spec.quantity, 2);
    assert_eq!(spec.color, Some(4));
    assert_eq!(spec.size, Some((2, 4)));
}

#[test]
fn a_part_phrase_needs_a_size_or_a_kind_to_count() {
    // A bare colour or count is how people talk, not a part.
    assert!(matches!(parse("red"), SentenceOutcome::Ignored));
    assert!(matches!(parse("2 beers"), SentenceOutcome::Ignored));
    assert!(matches!(parse("blue is nicer"), SentenceOutcome::Ignored));
}

#[test]
fn a_part_phrase_with_words_left_over_stays_silent() {
    // Half-understanding a sentence is the one thing the silence policy
    // cannot afford: say nothing rather than guess.
    assert!(matches!(
        parse("2x4 bricks are my favourite"),
        SentenceOutcome::Ignored
    ));
    assert!(matches!(
        parse("that 2x2 brick looks wrong"),
        SentenceOutcome::Ignored
    ));
}

#[test]
fn saying_how_they_sit_is_read_as_an_arrangement() {
    assert_eq!(
        proposal("2 2x2 bricks stacked vertically").1,
        Some(Arrangement::Stacked)
    );
    assert_eq!(
        proposal("3 2x2 bricks in a row").1,
        Some(Arrangement::RowEast)
    );
    assert_eq!(
        proposal("2 2x4 plates side by side").1,
        Some(Arrangement::RowEast)
    );
    assert_eq!(
        proposal("2 2x2 bricks north").1,
        Some(Arrangement::RowSouth)
    );
}

#[test]
fn a_rejected_sentence_never_quotes_a_rust_type_name() {
    // Each of these stops the parser on a token of a different kind, which
    // is where a `{:?}` used to leak `verb(add)` or `keyword(at)` into the
    // message shown in the thread.
    for sentence in [
        "Add the",
        "Add at",
        "Add north",
        "Add 5",
        "Add stud",
        "Add red",
        "Paint",
        "Replace",
        "Select",
        "Group",
        "Revert to",
    ] {
        let SentenceOutcome::Error(error) = parse(sentence) else {
            continue;
        };
        if let Some(found) = &error.found {
            assert!(
                !found.contains('(') && !found.contains('{'),
                "{sentence:?} quoted a debug value: {found:?}"
            );
            assert!(!found.is_empty(), "{sentence:?} quoted an empty token");
        }
    }
}

#[test]
fn a_quotable_token_is_still_quoted_back() {
    let SentenceOutcome::Error(error) = parse("Add zzz qqq") else {
        panic!("'Add zzz qqq' should be rejected");
    };
    assert_eq!(error.found.as_deref(), Some("zzz"));

    // A typed token maps back to the word it was written as, rather than
    // to the name of the enum variant it lexed to.
    let SentenceOutcome::Error(error) = parse("Revert to north") else {
        panic!("'Revert to north' should be rejected");
    };
    assert_eq!(error.found.as_deref(), Some("north"));
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
        assert_eq!(error.expected, Expectation::RightAngle);
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

// --- Drawings ----------------------------------------------------------

/// A whole message, so the fenced block reaches the sentence in front of
/// it. `parse` above goes through `tokenize_sentence`, which has no block.
fn first(text: &str) -> SentenceOutcome {
    super::parse_message(text)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no sentence in {text:?}"))
}

fn drawing(header: &str) -> GridSpec {
    let text = format!("{header}\n```\nb = blue 1x1 brick\n\n..bb..\n```");
    match first(&text) {
        SentenceOutcome::Statement(Statement::Command(c)) => match c.object {
            ObjectExpr::Grid(spec) => *spec,
            other => panic!("expected a drawing from {header:?}, got {other:?}"),
        },
        other => panic!("expected a command from {header:?}, got {other:?}"),
    }
}

#[test]
fn parses_a_grid_header_with_a_view_and_a_location() {
    let text = "Build from top at origin:\n```\nb = blue 1x1 brick\n\n..bb..\n```";
    let SentenceOutcome::Statement(Statement::Command(c)) = first(text) else {
        panic!("expected a command");
    };
    // The location comes from the ordinary grammar, not a second one.
    assert_eq!(c.location, Some(Location::At(Place::Origin)));
    match c.object {
        ObjectExpr::Grid(spec) => {
            assert_eq!(spec.view, GridView::Top);
            assert_eq!(spec.legend.len(), 1);
            assert_eq!(spec.legend[0].symbol, 'b');
            assert_eq!(spec.legend[0].spec.color, Some(1));
            assert_eq!(spec.layers.len(), 1);
            assert_eq!(spec.layers[0].rows[0], "..bb..".chars().collect::<Vec<_>>());
        }
        other => panic!("expected a drawing, got {other:?}"),
    }
}

#[test]
fn parses_every_view_word_including_the_ones_that_lex_as_directions() {
    // `top` and `bottom` are bare words; the compass points and
    // `above`/`below` lex as directions, so both kinds must be accepted.
    for (header, expected) in [
        ("Build from top:", GridView::Top),
        ("Build from the top view:", GridView::Top),
        ("Build top view:", GridView::Top),
        ("Build from bottom:", GridView::Bottom),
        ("Build from north:", GridView::North),
        ("Build from south:", GridView::South),
        ("Build from east:", GridView::East),
        ("Build from west:", GridView::West),
        ("Build from above:", GridView::Top),
        ("Build from below:", GridView::Bottom),
        ("Make from east at gate:", GridView::East),
    ] {
        assert_eq!(drawing(header).view, expected, "header: {header}");
    }
}

#[test]
fn a_build_header_with_no_view_is_reported() {
    let text = "Build from sideways:\n```\nb = blue 1x1 brick\n\n..bb..\n```";
    let SentenceOutcome::Error(error) = first(text) else {
        panic!("a misspelled view should be reported");
    };
    assert_eq!(error.expected, Expectation::ViewName);
}

#[test]
fn a_structure_sentence_that_happens_to_carry_a_block_still_builds_the_structure() {
    // A fence pasted under an ordinary command must not turn it into a
    // drawing.
    let text = "Build a wall 8 studs long.\n```\nsome code\n```";
    let SentenceOutcome::Statement(Statement::Command(c)) = first(text) else {
        panic!("expected a command");
    };
    assert_eq!(c.object, ObjectExpr::Structure(Structure::Wall));
}

#[test]
fn a_grid_header_without_a_block_is_not_a_drawing() {
    // Nothing to draw, so it falls back to the structure grammar and is
    // reported there.
    assert!(matches!(
        first("Build from top."),
        SentenceOutcome::Error(_)
    ));
}

#[test]
fn the_legend_keeps_the_order_it_was_written_in() {
    // Resolution order fixes which part a pending lookup answers, so it
    // must not depend on anything but what was typed.
    let text = "Build from top:\n```\nr = red 1x1 brick\nb = blue 1x1 brick\n\nrb\n```";
    let SentenceOutcome::Statement(Statement::Command(c)) = first(text) else {
        panic!("expected a command");
    };
    let ObjectExpr::Grid(spec) = c.object else {
        panic!("expected a drawing");
    };
    let symbols: Vec<char> = spec.legend.iter().map(|e| e.symbol).collect();
    assert_eq!(symbols, vec!['r', 'b']);
}

#[test]
fn a_badly_keyed_legend_line_is_carried_through_to_lowering() {
    let text = "Build from top:\n```\nbb = blue 1x1 brick\n\n..bb..\n```";
    let SentenceOutcome::Statement(Statement::Command(c)) = first(text) else {
        panic!("expected a command");
    };
    let ObjectExpr::Grid(spec) = c.object else {
        panic!("expected a drawing");
    };
    assert_eq!(spec.bad_legend_key.as_deref(), Some("bb"));
}

#[test]
fn parses_show_view_in_both_word_orders() {
    for (text, expected) in [
        ("Show top view", GridView::Top),
        ("Show the north view", GridView::North),
        ("Show the view from the east", GridView::East),
        ("Show bottom view", GridView::Bottom),
    ] {
        assert_eq!(
            statement(text),
            Statement::Query(Query::ShowView {
                view: expected,
                layer: None
            }),
            "text: {text}"
        );
    }
}

#[test]
fn parses_a_layer_selector_on_a_view_query() {
    assert_eq!(
        statement("Show top view layer 2"),
        Statement::Query(Query::ShowView {
            view: GridView::Top,
            layer: Some(2)
        })
    );
    assert_eq!(
        statement("Show the north view slice 3"),
        Statement::Query(Query::ShowView {
            view: GridView::North,
            layer: Some(3)
        })
    );
}

#[test]
fn the_other_show_forms_are_untouched_by_the_view_grammar() {
    assert_eq!(statement("Show parts"), Statement::Query(Query::ListParts));
    assert_eq!(
        statement("Show unsupported"),
        Statement::Query(Query::ShowUnsupported)
    );
    assert_eq!(
        statement("Show collisions"),
        Statement::Query(Query::FindCollisions)
    );
    // And something that names no view at all is still reported.
    assert!(matches!(parse("Show zzz"), SentenceOutcome::Error(_)));
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
    assert_eq!(statement("Help"), Statement::Meta(Meta::Help(None)));
    assert_eq!(statement("Finish"), Statement::Meta(Meta::Finish));
}

#[test]
fn help_names_a_topic_by_its_verb() {
    assert_eq!(
        statement("Help select"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Verb(Verb::Select))))
    );
    // A synonym still resolves to the canonical verb.
    assert_eq!(
        statement("Help put"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Verb(Verb::Add))))
    );
    // Anything else after "help" is ignored rather than reported.
    assert_eq!(statement("Help me"), Statement::Meta(Meta::Help(None)));
}

#[test]
fn help_names_a_topic_that_is_not_a_verb() {
    assert_eq!(
        statement("Help drawings"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Drawings)))
    );
    assert_eq!(
        statement("Help locations"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Locations)))
    );

    // "draw" and "colour" lex as `Render` and `Paint`, which is why the
    // topic is read from the sentence as typed: as tokens these two are
    // indistinguishable from the verbs they are synonyms of.
    assert_eq!(
        statement("Help draw"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Drawings)))
    );
    assert_eq!(
        statement("Help render"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Verb(Verb::Render))))
    );
    assert_eq!(
        statement("Help colours"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Parts)))
    );
    assert_eq!(
        statement("Help paint"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Verb(Verb::Paint))))
    );

    // The first word that names a topic wins, so filler in between is
    // skipped rather than losing the question.
    assert_eq!(
        statement("Help me with drawings"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Drawings)))
    );
    // `commands.` is an alias for `help.` and takes topics the same way.
    assert_eq!(
        statement("Commands drawings"),
        Statement::Meta(Meta::Help(Some(HelpTopic::Drawings)))
    );
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
