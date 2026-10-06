//! Parsing of command sentences (verb first).

use super::{
    Command, Direction, EMPTY_BLOCK, Expectation, Keyword, Modifier, ObjectExpr, ParseError,
    Parser, PartSpec, Ref, SentenceOutcome, Statement, Structure, Token, Unit, Verb, grid_spec,
};

pub(super) fn parse_name(parser: &mut Parser<'_>) -> SentenceOutcome {
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
        _ => return SentenceOutcome::Error(parser.error(Expectation::Name)),
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

pub(super) fn parse_command(parser: &mut Parser<'_>, verb: Verb) -> SentenceOutcome {
    let mut target_color = None;
    let mut target_part = None;
    // Modifiers recognised while parsing the object, which must survive into
    // the final command alongside the trailing ones.
    let mut extra_modifiers: Vec<Modifier> = Vec::new();

    let object = match verb {
        // "Build from top:" with a drawing after it. Only a sentence that
        // carries a block *and* names a view takes this path, so
        // `Build a wall 8 studs long.` with a stray fence after it still
        // builds a wall.
        Verb::Build if parser.has_block() && parser.looks_like_grid_header() => {
            parser.skip_filler();
            parser.eat_keyword(Keyword::From);
            parser.skip_filler();
            let Some(view) = parser.grid_view() else {
                return SentenceOutcome::Error(parser.error(Expectation::ViewName));
            };
            let _ = parser.eat_word("view");
            let block = parser.block.unwrap_or(&EMPTY_BLOCK);
            ObjectExpr::Grid(Box::new(grid_spec(view, block)))
        }
        Verb::Build => {
            parser.skip_filler();
            // "Build a wall ..." / "Build a row of 20 black tiles east."
            let structure = match parser.peek() {
                Some(Token::Word(w)) => {
                    Structure::parse(w).unwrap_or_else(|| Structure::Unknown(w.clone()))
                }
                Some(Token::Keyword(Keyword::Steps)) => Structure::Stairs,
                _ => return SentenceOutcome::Error(parser.error(Expectation::SomethingToBuild)),
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
                return SentenceOutcome::Error(parser.error(Expectation::PartToAdd));
            }
            ObjectExpr::Part(Box::new(spec))
        }
        Verb::Paint => {
            // The colour always comes last: "Paint selection dark tan.",
            // "Paint all red bricks blue." Claim it before parsing the object,
            // or the part description would absorb it.
            let Some(color_index) = parser.last_color_index(parser.pos) else {
                return SentenceOutcome::Error(parser.error(Expectation::ColourToPaintWith));
            };
            let Some(Token::Color(code)) = parser.tokens.get(color_index) else {
                return SentenceOutcome::Error(parser.error(Expectation::ColourToPaintWith));
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
                return SentenceOutcome::Error(parser.error(Expectation::SomethingToReplace));
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
                    return SentenceOutcome::Error(
                        parser.error(Expectation::ReplacementPartOrColour),
                    );
                }
                target_part = Some(Box::new(spec));
            }
            ObjectExpr::Reference(reference)
        }
        Verb::Select => {
            let Some(reference) = parser.reference() else {
                return SentenceOutcome::Error(parser.error(Expectation::SomethingToSelect));
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
                _ => return SentenceOutcome::Error(parser.error(Expectation::GroupName)),
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
            expected: Expectation::RightAngle,
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
