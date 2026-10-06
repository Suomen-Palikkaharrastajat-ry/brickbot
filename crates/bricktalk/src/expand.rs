//! Expanding high-level building intents into individual placements.
//!
//! `Build a wall 10 studs long 6 bricks high.` becomes sixty ordinary
//! placements, each individually addressable afterwards — so `Paint that brick
//! blue.` and `Mirror the wall across the centre.` work on them normally. The
//! expansion is not a black box: it produces the same operations a builder
//! could have typed by hand, tagged with the structure's name so they can be
//! referred to as a group.

#![allow(clippy::doc_markdown)]

use super::dsl::ast::{Command, ExtentAxis, ObjectExpr, Ref, Structure};
use super::dsl::lexer::Verb;
use super::lower::{
    LowerError, Unsupported, checked_total, extent_count, resolve_ref, structure_group,
    structure_part,
};
use super::model::{Model, PartRef, Placement};
use super::ops::{Op, Placed};
use super::units::{Direction, IVec3, LDU_STUD, Orientation, Unit};

/// Where a structure starts from: on top of whatever was last touched, or at
/// the origin if nothing has been built.
fn anchor(model: &Model) -> IVec3 {
    model
        .last_ref
        .last()
        .and_then(|id| model.get(*id))
        .map_or(IVec3::ZERO, Placement::stack_position)
}

/// The horizontal extent of a part along a direction, in LDraw units.
fn footprint_span(part: &PartRef, direction: Direction) -> i32 {
    let span = match direction {
        Direction::East | Direction::West => i32::from(part.footprint.0),
        _ => i32::from(part.footprint.1),
    };
    (span * LDU_STUD).max(LDU_STUD)
}

/// The direction a structure runs in, defaulting to east.
fn run_direction(command: &Command) -> Direction {
    command
        .distance()
        .map(|(direction, ..)| direction)
        .filter(|d| !d.is_vertical())
        .unwrap_or(Direction::East)
}

/// Expands a `build` command into placement operations.
#[allow(clippy::too_many_lines)]
pub fn expand(
    model: &Model,
    command: &Command,
    structure: &Structure,
    chosen: Option<&PartRef>,
) -> Result<Vec<Op>, LowerError> {
    if let Structure::Unknown(name) = structure {
        return Err(LowerError::UnknownStructure(name.clone()));
    }

    let part = structure_part(command, chosen)?;
    let color = command
        .using()
        .and_then(|spec| spec.color)
        .or(command.target_color)
        .unwrap_or(super::lower::DEFAULT_COLOR);
    let group = Some(structure_group(structure));
    let start = anchor(model);
    let direction = run_direction(command);
    let step = footprint_span(&part, direction);
    let course_height = part.kind.height_ldu();

    // Each branch collects placements; the whole structure becomes one
    // `AddMany`, so it is a single undo step and every brick still gets its own
    // sequence number within the revision.
    let make = |pos: IVec3, orientation: Orientation| Placed {
        part: part.clone(),
        color,
        pos,
        orientation,
    };
    let finish = |placements: Vec<Placed>| {
        vec![Op::AddMany {
            placements,
            group: group.clone(),
        }]
    };

    match structure {
        Structure::Row => {
            // "Build a row of 20 black tiles east."
            let count = command
                .quantity()
                .or_else(|| command.using().map(|s| s.quantity))
                .filter(|n| *n > 1)
                .unwrap_or_else(|| extent_count(command, ExtentAxis::Length, 10));
            let count = checked_total(count)?;
            Ok(finish(
                (0..count)
                    .map(|i| {
                        #[allow(clippy::cast_possible_wrap)]
                        let offset = direction.step().scale(step * i as i32);
                        make(start.add(offset), Orientation::DEFAULT)
                    })
                    .collect(),
            ))
        }

        Structure::Wall => {
            // Length is in studs; height is in courses of the chosen part.
            let length_studs = extent_count(command, ExtentAxis::Length, 8);
            let height = extent_count(command, ExtentAxis::Height, 3);
            let per_course = studs_to_count(length_studs, &part, direction);
            checked_total(per_course * height)?;

            // Running bond: alternate courses shift half a part along, which
            // is how a real wall is laid and stops the seams lining up. Half a
            // part has to come out as whole studs, though — a course shifted
            // half a stud has no studs under it at all. A part only one stud
            // long the way the wall runs simply cannot be bonded, so those
            // courses line up.
            let bond = if (step / 2) % LDU_STUD == 0 {
                step / 2
            } else {
                0
            };

            let mut ops = Vec::new();
            for course in 0..height {
                for i in 0..per_course {
                    #[allow(clippy::cast_possible_wrap)]
                    let along = step * i as i32 + if course % 2 == 1 { bond } else { 0 };
                    #[allow(clippy::cast_possible_wrap)]
                    let up = IVec3::new(0, -course_height * course as i32, 0);
                    let pos = start.add(direction.step().scale(along)).add(up);
                    ops.push(make(pos, Orientation::DEFAULT));
                }
            }
            Ok(finish(ops))
        }

        Structure::Tower => {
            let height = extent_count(command, ExtentAxis::Height, 5);
            let height = checked_total(height)?;
            Ok(finish(
                (0..height)
                    .map(|i| {
                        #[allow(clippy::cast_possible_wrap)]
                        let up = IVec3::new(0, -course_height * i as i32, 0);
                        make(start.add(up), Orientation::DEFAULT)
                    })
                    .collect(),
            ))
        }

        Structure::Floor => {
            let width_studs = extent_count(command, ExtentAxis::Width, 8);
            let depth_studs = extent_count(command, ExtentAxis::Depth, 8);
            let across = studs_to_count(width_studs, &part, Direction::East);
            let along = studs_to_count(depth_studs, &part, Direction::South);
            checked_total(across * along)?;

            let step_x = footprint_span(&part, Direction::East);
            let step_z = footprint_span(&part, Direction::South);
            let mut ops = Vec::new();
            for row in 0..along {
                for col in 0..across {
                    #[allow(clippy::cast_possible_wrap)]
                    let pos = start.add(IVec3::new(step_x * col as i32, 0, step_z * row as i32));
                    ops.push(make(pos, Orientation::DEFAULT));
                }
            }
            Ok(finish(ops))
        }

        Structure::Box => {
            // A hollow enclosure: four walls, one course high per level.
            let width_studs = extent_count(command, ExtentAxis::Width, 8);
            let depth_studs = extent_count(command, ExtentAxis::Depth, 8);
            let height = extent_count(command, ExtentAxis::Height, 3);
            let across = studs_to_count(width_studs, &part, Direction::East);
            let along = studs_to_count(depth_studs, &part, Direction::South);
            checked_total((across + along) * 2 * height)?;

            let step_x = footprint_span(&part, Direction::East);
            let step_z = footprint_span(&part, Direction::South);
            let mut ops = Vec::new();
            for course in 0..height {
                #[allow(clippy::cast_possible_wrap)]
                let up = IVec3::new(0, -course_height * course as i32, 0);
                for col in 0..across {
                    #[allow(clippy::cast_possible_wrap)]
                    let x = step_x * col as i32;
                    #[allow(clippy::cast_possible_wrap)]
                    let far = step_z * (along.saturating_sub(1)) as i32;
                    ops.push(make(
                        start.add(IVec3::new(x, 0, 0)).add(up),
                        Orientation::DEFAULT,
                    ));
                    if along > 1 {
                        ops.push(make(
                            start.add(IVec3::new(x, 0, far)).add(up),
                            Orientation::DEFAULT,
                        ));
                    }
                }
                for row in 1..along.saturating_sub(1) {
                    #[allow(clippy::cast_possible_wrap)]
                    let z = step_z * row as i32;
                    #[allow(clippy::cast_possible_wrap)]
                    let far = step_x * (across.saturating_sub(1)) as i32;
                    ops.push(make(
                        start.add(IVec3::new(0, 0, z)).add(up),
                        Orientation::DEFAULT,
                    ));
                    if across > 1 {
                        ops.push(make(
                            start.add(IVec3::new(far, 0, z)).add(up),
                            Orientation::DEFAULT,
                        ));
                    }
                }
            }
            Ok(finish(ops))
        }

        Structure::Stairs => {
            let steps = command
                .quantity()
                .unwrap_or_else(|| extent_count(command, ExtentAxis::Height, 5));
            let steps = checked_total(steps)?;
            // A tread has to sit *on* the one below it, so the run is one stud
            // shorter than the part: stepping clear by a whole part leaves the
            // two merely touching at the edge, which holds nothing up. A part
            // only one stud long the way we are going cannot overlap at all,
            // so it falls back to a stud and the caller's settling deals with
            // what is left.
            let run = (step - LDU_STUD).max(LDU_STUD);
            Ok(finish(
                (0..steps)
                    .map(|i| {
                        #[allow(clippy::cast_possible_wrap)]
                        let along = direction.step().scale(run * i as i32);
                        #[allow(clippy::cast_possible_wrap)]
                        let up = IVec3::new(0, -course_height * i as i32, 0);
                        make(start.add(along).add(up), Orientation::DEFAULT)
                    })
                    .collect(),
            ))
        }

        Structure::Unknown(_) => unreachable!("handled above"),
    }
}

/// How many copies of a part are needed to span a length in studs.
fn studs_to_count(studs: u32, part: &PartRef, direction: Direction) -> u32 {
    let per_part = match direction {
        Direction::East | Direction::West => u32::from(part.footprint.0),
        _ => u32::from(part.footprint.1),
    }
    .max(1);
    studs.div_ceil(per_part).max(1)
}

/// Expands `duplicate`, `mirror` and `repeat`, all of which copy existing
/// placements rather than creating new shapes.
pub fn expand_copy(model: &Model, command: &Command) -> Result<Vec<Op>, LowerError> {
    let reference = match &command.object {
        ObjectExpr::Reference(reference) => reference.clone(),
        ObjectExpr::None => Ref::Last,
        _ => return Err(LowerError::Unsupported(Unsupported::That)),
    };
    let targets = resolve_ref(model, &reference)?;
    let sources: Vec<&Placement> = targets.iter().filter_map(|id| model.get(*id)).collect();
    if sources.is_empty() {
        return Err(LowerError::NothingToActOn);
    }

    match command.verb {
        Verb::Mirror => {
            // Reflect across the model's centre on the axis given, or east-west
            // by default.
            let axis = command
                .distance()
                .map(|(direction, ..)| direction)
                .filter(|d| !d.is_vertical())
                .unwrap_or(Direction::East);
            let (min, max) = model.bounds().ok_or(LowerError::NothingToActOn)?;
            let centre_x = min.x.midpoint(max.x);
            let centre_z = min.z.midpoint(max.z);

            let placements = sources
                .iter()
                .map(|p| {
                    let pos = match axis {
                        Direction::East | Direction::West => {
                            IVec3::new(2 * centre_x - p.pos.x, p.pos.y, p.pos.z)
                        }
                        _ => IVec3::new(p.pos.x, p.pos.y, 2 * centre_z - p.pos.z),
                    };
                    Placed {
                        part: p.part.clone(),
                        color: p.color,
                        pos,
                        // Mirroring turns a part to face the other way.
                        orientation: p.orientation.turned(2),
                    }
                })
                .collect();
            Ok(vec![Op::AddMany {
                placements,
                group: sources.first().and_then(|p| p.group.clone()),
            }])
        }

        Verb::Duplicate | Verb::Repeat => {
            let copies = command.quantity().unwrap_or(1).clamp(1, 50);
            #[allow(clippy::cast_possible_truncation)]
            let total = (sources.len() as u32).saturating_mul(copies);
            checked_total(total)?;

            // Offset by the copied group's own width, so copies sit alongside
            // rather than on top of the original. Measuring the whole model
            // instead — which is what this used to do — flings a small group
            // clear across a large build.
            let offset = command.distance().map_or_else(
                || {
                    let (min, max) = model
                        .subset_bounds(&targets)
                        .unwrap_or((IVec3::ZERO, IVec3::ZERO));
                    IVec3::new((max.x - min.x).max(LDU_STUD), 0, 0)
                },
                |(direction, count, unit)| direction.offset(count.max(1), unit),
            );

            let mut placements = Vec::new();
            for copy in 1..=copies {
                for p in &sources {
                    #[allow(clippy::cast_possible_wrap)]
                    let pos = p.pos.add(offset.scale(copy as i32));
                    placements.push(Placed {
                        part: p.part.clone(),
                        color: p.color,
                        pos,
                        orientation: p.orientation,
                    });
                }
            }
            Ok(vec![Op::AddMany {
                placements,
                group: sources.first().and_then(|p| p.group.clone()),
            }])
        }

        _ => Err(LowerError::Unsupported(Unsupported::Verb)),
    }
}

/// The default unit an extent is measured in, per axis.
#[must_use]
pub const fn default_extent_unit(axis: ExtentAxis) -> Unit {
    match axis {
        ExtentAxis::Height => Unit::Brick,
        _ => Unit::Stud,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::Statement;
    use crate::dsl::lexer::tokenize_sentence;
    use crate::dsl::parser::{SentenceOutcome, parse_sentence};
    use crate::lower::lower_command;
    use crate::ops::apply;

    fn command_of(text: &str) -> Command {
        match parse_sentence(&tokenize_sentence(text)) {
            SentenceOutcome::Statement(Statement::Command(c)) => *c,
            other => panic!("expected a command from {text:?}, got {other:?}"),
        }
    }

    fn run(sentences: &[&str]) -> Model {
        let mut model = Model::new("test");
        for (rev, text) in sentences.iter().enumerate() {
            let command = command_of(text);
            let ops = lower_command(&model, &command)
                .unwrap_or_else(|e| panic!("{text:?} failed to lower: {e}"));
            #[allow(clippy::cast_possible_truncation)]
            for op in &ops {
                apply(&mut model, rev as u32, op);
            }
        }
        model
    }

    #[test]
    fn builds_a_wall_of_the_requested_size() {
        let model = run(&["Build a wall 8 studs long 3 bricks high"]);
        // Eight studs of 2-wide bricks is four per course, three courses.
        assert_eq!(model.len(), 12);
        assert_eq!(model.height_in_bricks(), 3);
    }

    #[test]
    fn wall_courses_are_laid_in_a_running_bond() {
        let model = run(&["Build a wall 8 studs long 2 bricks high"]);
        let first_course: Vec<i32> = model
            .placements
            .iter()
            .filter(|p| p.pos.y == 0)
            .map(|p| p.pos.x)
            .collect();
        let second_course: Vec<i32> = model
            .placements
            .iter()
            .filter(|p| p.pos.y == -24)
            .map(|p| p.pos.x)
            .collect();
        assert!(!first_course.is_empty());
        assert!(!second_course.is_empty());
        // The second course is offset, so the seams do not line up.
        assert_ne!(first_course, second_course);
    }

    #[test]
    fn every_wall_brick_belongs_to_the_wall_group() {
        let model = run(&["Build a wall 4 studs long 1 brick high"]);
        assert!(
            model
                .placements
                .iter()
                .all(|p| p.group.as_deref() == Some("wall"))
        );
        assert_eq!(
            model.resolve_group("wall").map(|ids| ids.len()),
            Some(model.len())
        );
    }

    #[test]
    fn every_brick_in_a_structure_gets_a_distinct_id() {
        // Regression: expansion used to emit one op per brick, so every brick
        // claimed the same PlacementId and later edits hit only one of them.
        let model = run(&["Build a wall 8 studs long 3 bricks high"]);
        let ids: std::collections::HashSet<_> = model.placements.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), model.len(), "placement ids must be distinct");
    }

    #[test]
    fn a_structure_is_a_single_undo_step() {
        let mut history = crate::ops::History::new("t");
        let command = command_of("Build a wall 8 studs long 3 bricks high");
        for op in lower_command(&history.materialize(), &command).unwrap() {
            history.push(op, "alice");
        }
        assert_eq!(history.materialize().len(), 12);
        history.undo();
        assert!(
            history.materialize().is_empty(),
            "one undo removes the wall"
        );
    }

    #[test]
    fn an_expanded_wall_stays_individually_editable() {
        let model = run(&[
            "Build a wall 4 studs long 1 brick high",
            "Paint all bricks dark tan",
        ]);
        assert!(model.placements.iter().all(|p| p.color == 28));
    }

    #[test]
    fn builds_a_row_of_the_requested_length() {
        let model = run(&["Build a row of 6 black tiles east"]);
        assert_eq!(model.len(), 6);
        // Laid out along the east axis.
        let xs: Vec<i32> = model.placements.iter().map(|p| p.pos.x).collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn builds_a_tower_upwards() {
        let model = run(&["Build a tower 4 bricks high"]);
        assert_eq!(model.len(), 4);
        assert_eq!(model.height_in_bricks(), 4);
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn builds_a_floor_across_two_axes() {
        let model = run(&["Build a floor 8 wide 8 deep using tan plates"]);
        assert!(
            model
                .placements
                .iter()
                .all(|p| p.part.kind == crate::model::PartKind::Plate)
        );
        assert!(model.collisions().is_empty());

        // Assert the coverage, not a piece count: how many parts it takes
        // depends on the element's footprint, and a 2x4 plate tiles 8x8 studs
        // with eight pieces rather than sixty-four.
        let (min, max) = model.bounds().unwrap();
        assert_eq!(max.x - min.x, 8 * LDU_STUD);
        assert_eq!(max.z - min.z, 8 * LDU_STUD);
    }

    #[test]
    fn builds_stairs_that_rise_as_they_go() {
        let model = run(&["Build stairs 4 steps"]);
        assert!(model.len() >= 2);
        let mut sorted: Vec<_> = model.placements.iter().collect();
        sorted.sort_by_key(|p| p.pos.x);
        // Each step is higher (smaller Y) than the one before it.
        assert!(sorted.windows(2).all(|w| w[1].pos.y <= w[0].pos.y));
    }

    #[test]
    fn a_wall_of_one_wide_bricks_still_bonds_on_whole_studs() {
        // Half of a two-stud part is one stud, which bonds. Half of a one-stud
        // part is half a stud, which is not a thing you can build: the course
        // above would have no studs under it.
        // Where the wall as a whole sits is the caller's business; that its
        // own bricks line up with each other is this expander's.
        let model = run(&["Build a wall 6 studs long 2 bricks high using 1x2 bricks"]);
        assert!(model.collisions().is_empty());
        let base = model.placements[0].pos.x;
        for p in &model.placements {
            assert_eq!(
                (p.pos.x - base).rem_euclid(LDU_STUD),
                0,
                "{:?} is bonded half a stud off the rest of the wall",
                p.pos
            );
        }
    }

    #[test]
    fn every_step_of_a_staircase_rests_on_the_one_below_it() {
        // Stepping clear by a whole part left each tread merely touching the
        // edge of its predecessor, which holds nothing up: every tread above
        // the first was floating in mid-air.
        let model = run(&["Build stairs 4 steps"]);
        assert!(
            model.unsupported().is_empty(),
            "a staircase must be climbable"
        );
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn a_box_is_hollow() {
        // Large enough that the perimeter is genuinely smaller than the area.
        // At 6x6 with 2x4 bricks the two are equal, which would prove nothing.
        let hollow = run(&["Build a box 20 wide 20 deep 1 brick high"]);
        let solid = run(&["Build a floor 20 wide 20 deep"]);
        assert!(
            hollow.len() < solid.len(),
            "an enclosure ({}) should use fewer parts than a solid floor ({})",
            hollow.len(),
            solid.len()
        );
        // ... while still spanning the same footprint.
        assert_eq!(
            hollow.bounds().map(|(min, max)| max.x - min.x),
            solid.bounds().map(|(min, max)| max.x - min.x)
        );
    }

    #[test]
    fn an_unknown_structure_is_reported_rather_than_guessed() {
        let model = Model::new("t");
        let command = command_of("Build a gazebo");
        assert_eq!(
            lower_command(&model, &command),
            Err(LowerError::UnknownStructure("gazebo".to_string()))
        );
    }

    #[test]
    fn an_absurdly_large_structure_is_refused() {
        let model = Model::new("t");
        let command = command_of("Build a floor 500 wide 500 deep");
        assert!(matches!(
            lower_command(&model, &command),
            Err(LowerError::Unsupported(_))
        ));
    }

    #[test]
    fn mirroring_doubles_the_model_across_the_centre() {
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Add a red 2x4 brick at 100 0 0",
            "Mirror everything",
        ]);
        assert_eq!(model.len(), 4);
    }

    #[test]
    fn duplicating_offsets_the_copy_clear_of_the_original() {
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Call this tower",
            "Duplicate tower east 4 studs",
        ]);
        assert_eq!(model.len(), 2);
        assert_eq!(model.placements[1].pos, IVec3::new(80, 0, 0));
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn mirroring_takes_its_axis_from_the_sentence() {
        // Regression: "across the centre" was swallowed by the modifier
        // parser's skip-the-token fallback, so every mirror used the default
        // east-west axis whatever the sentence said.
        let across_z = run(&[
            "Add a red 2x4 brick at origin",
            "Add a red 2x4 brick at 0 0 200",
            "Mirror everything across north",
        ]);
        // Reflecting north-south moves parts along Z and leaves X alone.
        assert_eq!(across_z.len(), 4);
        assert!(
            across_z.placements[2..].iter().all(|p| p.pos.x == 0),
            "a north-south mirror must not move anything east or west"
        );

        // "across the centre" names the pivot, not an axis, and still works.
        let default_axis = run(&[
            "Add a red 2x4 brick at origin",
            "Add a red 2x4 brick at 200 0 0",
            "Mirror everything across the centre",
        ]);
        assert_eq!(default_axis.len(), 4);
    }

    #[test]
    fn duplicating_a_group_steps_by_the_group_not_the_whole_model() {
        // Regression: the default offset came from the *model's* bounds, so
        // duplicating a single brick inside a large build flung the copy right
        // across it instead of setting it alongside.
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Call this tower",
            // A wide, unrelated structure that must not affect the step.
            "Add a blue 2x4 brick at 2000 0 0",
            "Duplicate tower",
        ]);

        let original = model.resolve_group("tower").unwrap()[0];
        let original = model.get(original).unwrap().pos;
        let copy = model.placements.last().unwrap().pos;
        assert_eq!(
            copy.x - original.x,
            40,
            "one 2x4 brick's width east, not the model's 2000 LDU span"
        );
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn duplicating_a_single_brick_with_no_footprint_still_moves_it_clear() {
        let model = run(&["Add a red 2x4 brick at origin", "Duplicate that"]);
        assert_eq!(model.len(), 2);
        assert!(
            model.collisions().is_empty(),
            "a copy on top of the original would be useless"
        );
    }

    #[test]
    fn duplicating_with_nothing_to_copy_is_reported() {
        let model = Model::new("t");
        let command = command_of("Duplicate everything");
        assert_eq!(
            lower_command(&model, &command),
            Err(LowerError::NothingToActOn)
        );
    }

    #[test]
    fn studs_convert_to_part_counts_by_rounding_up() {
        let brick = crate::parts::lookup(crate::model::PartKind::Brick, 2, 4).unwrap();
        // A 2-wide brick spans 2 studs, so 8 studs needs 4 of them.
        assert_eq!(studs_to_count(8, &brick, Direction::East), 4);
        // 7 studs still needs 4, rounded up.
        assert_eq!(studs_to_count(7, &brick, Direction::East), 4);
        assert_eq!(studs_to_count(0, &brick, Direction::East), 1);
    }

    #[test]
    fn default_extent_units_match_how_builders_measure() {
        assert_eq!(default_extent_unit(ExtentAxis::Height), Unit::Brick);
        assert_eq!(default_extent_unit(ExtentAxis::Length), Unit::Stud);
    }
}
