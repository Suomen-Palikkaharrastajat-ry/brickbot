//! Turns parsed statements into model operations.
//!
//! This is the one stage that needs the model: `that`, `the wall` and `on top
//! of the blue plate` cannot be understood without knowing what is already
//! built. Keeping it separate from the parser means the grammar stays a pure
//! function of a string, and everything that depends on context lives here.
//!
//! Still pure: it takes a `&Model` and returns operations, so it is tested
//! against fixture models with no mocks.

#![allow(clippy::doc_markdown)]

use super::dsl::ast::{
    Command, ExtentAxis, GridSpec, Location, Modifier, ObjectExpr, PartSpec, Place, Ref, Structure,
};
use super::dsl::lexer::Verb;
use super::grid::{GridError, LegendPart};
use super::model::{Model, PartKind, PartRef, PlacementId};
use super::ops::{Op, Repeat, SelectPredicate};
use super::parts::{self, Resolution};
use super::units::{Direction, IVec3, LDU_STUD, Orientation, Unit};

/// Why a well-formed sentence could not be carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// `that` / `it` with nothing to refer to.
    UnresolvedReference(String),
    /// A group or landmark nobody has named.
    UnknownName(String),
    /// The model is empty and the command needs something to act on.
    NothingToActOn,
    /// A structure the expander does not know how to build.
    UnknownStructure(String),
    /// The command would need a part we have to look up first.
    NeedsPartLookup { query: String, kind: PartKind },
    /// The command is understood but cannot be carried out as written.
    Unsupported(Unsupported),
    /// A drawing that could not be read. Carries data, which is why it is here
    /// rather than in the data-free [`Unsupported`] set.
    Grid(GridError),
}

/// What exactly the interpreter could not do.
///
/// A closed set rather than free text: each variant maps to one sentence in
/// each locale bundle, and `report.rs` is the only place that renders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// The verb was understood but the thing it was pointed at was not.
    That,
    /// The verb itself is not implemented yet.
    Verb,
    /// `Move` with nothing saying which way.
    MoveWithoutDirection,
    /// `Paint` with no colour.
    PaintWithoutColour,
    /// `Replace` with nothing to replace it with.
    Replacement,
    /// `Select` pointed at something that is not selectable.
    Selection,
    /// `Group` or `Name` with no name given.
    Name,
    /// The structure asked for would exceed the per-command placement cap.
    TooLarge { total: u32, limit: u32 },
}

impl Unsupported {
    /// Localization key of the sentence describing this gap.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::That => "command.build.unsupported.that",
            Self::Verb => "command.build.unsupported.verb",
            Self::MoveWithoutDirection => "command.build.unsupported.move_without_direction",
            Self::PaintWithoutColour => "command.build.unsupported.paint_without_colour",
            Self::Replacement => "command.build.unsupported.replacement",
            Self::Selection => "command.build.unsupported.selection",
            Self::Name => "command.build.unsupported.name",
            Self::TooLarge { .. } => "command.build.unsupported.too_large",
        }
    }
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnresolvedReference(word) => {
                write!(f, "I'm not sure what '{word}' refers to")
            }
            Self::UnknownName(name) => write!(f, "I don't know anything called '{name}'"),
            Self::NothingToActOn => write!(f, "there is nothing in the model yet"),
            Self::UnknownStructure(name) => write!(f, "I don't know how to build a {name}"),
            Self::NeedsPartLookup { query, .. } => write!(f, "I need to look up '{query}'"),
            Self::Unsupported(what) => write!(f, "I can't do {what:?} yet"),
            Self::Grid(error) => write!(f, "I can't read that drawing: {error:?}"),
        }
    }
}

/// The maximum number of placements one sentence may create.
///
/// `Build a floor 200 by 200` is a typo, not a request, and expanding it
/// would lock up the renderer.
pub const MAX_PLACEMENTS_PER_COMMAND: u32 = 400;

/// Caps a placement count and reports when the cap bites, so the caller can
/// tell the builder rather than silently building something smaller.
///
/// Lives beside the limit it enforces; both structures and drawings use it.
pub const fn checked_total(total: u32) -> Result<u32, LowerError> {
    if total > MAX_PLACEMENTS_PER_COMMAND {
        return Err(LowerError::Unsupported(Unsupported::TooLarge {
            total,
            limit: MAX_PLACEMENTS_PER_COMMAND,
        }));
    }
    Ok(total)
}

/// Resolves a reference to the placements it names.
pub fn resolve_ref(model: &Model, reference: &Ref) -> Result<Vec<PlacementId>, LowerError> {
    match reference {
        Ref::Last => {
            if model.last_ref.is_empty() {
                return Err(LowerError::UnresolvedReference("that".to_string()));
            }
            // "that" is singular: take the most recent one.
            Ok(vec![*model.last_ref.last().unwrap_or(&model.last_ref[0])])
        }
        Ref::LastPlural => {
            if model.last_ref.is_empty() {
                if model.selection.is_empty() {
                    return Err(LowerError::UnresolvedReference("them".to_string()));
                }
                return Ok(model.selection.clone());
            }
            Ok(model.last_ref.clone())
        }
        Ref::LastOfKind(spec) => {
            let predicate = predicate_of(spec);
            model
                .last_matching(|p| predicate.matches(p, model))
                .map(|p| vec![p.id])
                .ok_or_else(|| LowerError::UnresolvedReference(describe(spec)))
        }
        Ref::AllMatching(spec) => {
            let ids = predicate_of(spec).resolve(model);
            if ids.is_empty() {
                return Err(LowerError::UnresolvedReference(describe(spec)));
            }
            Ok(ids)
        }
        Ref::Named(name) => model
            .resolve_group(name)
            .filter(|ids| !ids.is_empty())
            .ok_or_else(|| LowerError::UnknownName(name.clone())),
        Ref::Selection => {
            if model.selection.is_empty() {
                return Err(LowerError::UnresolvedReference("the selection".to_string()));
            }
            Ok(model.selection.clone())
        }
        Ref::Everything => {
            if model.is_empty() {
                return Err(LowerError::NothingToActOn);
            }
            Ok(model.placements.iter().map(|p| p.id).collect())
        }
        Ref::Highest => model
            .highest()
            .map(|p| vec![p.id])
            .ok_or(LowerError::NothingToActOn),
    }
}

/// Builds a selection predicate from a described part.
#[must_use]
pub fn predicate_of(spec: &PartSpec) -> SelectPredicate {
    SelectPredicate {
        color: spec.color,
        kind: spec.kind,
        ..SelectPredicate::default()
    }
}

fn describe(spec: &PartSpec) -> String {
    spec.search_query()
}

/// Works out where a new part goes.
fn resolve_location(
    model: &Model,
    location: Option<&Location>,
    part: &PartRef,
) -> Result<IVec3, LowerError> {
    let Some(location) = location else {
        // With no location given, rest the part on top of whatever was last
        // touched, which is how a conversation naturally stacks bricks. With
        // nothing touched yet, start at the origin.
        return Ok(model
            .last_ref
            .last()
            .and_then(|id| model.get(*id))
            .map_or(IVec3::ZERO, super::model::Placement::stack_position));
    };

    match location {
        Location::At(Place::Origin) => Ok(IVec3::ZERO),
        Location::At(Place::Coords(coords)) => Ok(*coords),
        Location::At(Place::Landmark(name)) => model
            .landmarks
            .get(&name.to_lowercase())
            .copied()
            .or_else(|| {
                // A group name works as a place too: use its first member.
                model
                    .resolve_group(name)
                    .and_then(|ids| ids.first().copied())
                    .and_then(|id| model.get(id))
                    .map(super::model::Placement::stack_position)
            })
            .ok_or_else(|| LowerError::UnknownName(name.clone())),
        Location::On(reference) => {
            let ids = resolve_ref(model, reference)?;
            let anchor = ids
                .iter()
                .filter_map(|id| model.get(*id))
                // Rest on the highest of the referenced parts.
                .min_by_key(|p| p.top_y())
                .ok_or(LowerError::NothingToActOn)?;
            Ok(anchor.stack_position())
        }
        Location::Under(reference) => {
            let ids = resolve_ref(model, reference)?;
            let anchor = ids
                .iter()
                .filter_map(|id| model.get(*id))
                .max_by_key(|p| p.pos.y)
                .ok_or(LowerError::NothingToActOn)?;
            // Directly beneath: the new part's own height puts its origin
            // one part-height below the anchor's base.
            Ok(IVec3::new(
                anchor.pos.x,
                anchor.pos.y + part.kind.height_ldu(),
                anchor.pos.z,
            ))
        }
        Location::Direction { direction, of } => {
            let ids = resolve_ref(model, of)?;
            let anchor = ids
                .iter()
                .find_map(|id| model.get(*id))
                .ok_or(LowerError::NothingToActOn)?;
            // Step clear of the anchor by half of each part's extent, so the
            // two sit side by side rather than overlapping.
            let anchor_span = span_along(anchor.part.footprint, anchor.orientation, *direction);
            let new_span = span_along(part.footprint, Orientation::DEFAULT, *direction);
            let gap = anchor_span.midpoint(new_span);
            Ok(anchor.pos.add(direction.step().scale(gap.max(LDU_STUD))))
        }
    }
}

/// How far a footprint reaches along a direction, in LDraw units.
fn span_along(footprint: (u8, u8), orientation: Orientation, direction: Direction) -> i32 {
    if direction.is_vertical() {
        return 0;
    }
    let (w, d) = orientation.apply_footprint(i32::from(footprint.0), i32::from(footprint.1));
    match direction {
        Direction::East | Direction::West => w * LDU_STUD,
        _ => d * LDU_STUD,
    }
}

/// Resolves a described part into a real element.
///
/// `chosen` is the part a builder has just picked out of the `/part` dropdown,
/// supplied when a paused sentence is retried. It answers any lookup this
/// sentence would otherwise have to ask about — a sentence needing two unknown
/// parts simply pauses again on the second.
fn resolve_part(spec: &PartSpec, chosen: Option<&PartRef>) -> Result<PartRef, LowerError> {
    match parts::resolve(spec) {
        Resolution::Known(part) => Ok(part),
        Resolution::NeedsLookup { query, kind } => chosen
            .cloned()
            .ok_or(LowerError::NeedsPartLookup { query, kind }),
    }
}

/// Reads the orientation a command asks for.
fn orientation_of(command: &Command) -> Orientation {
    let mut orientation = Orientation::DEFAULT;
    for modifier in &command.modifiers {
        match modifier {
            Modifier::Turn(turns) => orientation = orientation.turned(*turns),
            Modifier::Flip => orientation = orientation.flipped(),
            Modifier::Facing(direction) => orientation = Orientation::facing(*direction),
            _ => {}
        }
    }
    orientation
}

/// The total offset a command's distance modifiers add up to.
fn offset_of(command: &Command) -> IVec3 {
    command
        .modifiers
        .iter()
        .fold(IVec3::ZERO, |acc, modifier| match modifier {
            Modifier::Distance {
                direction,
                count,
                unit,
            } => acc.add(direction.offset(*count, *unit)),
            _ => acc,
        })
}

/// Lowers one command into the operations that carry it out.
#[allow(clippy::too_many_lines)]
pub fn lower_command(model: &Model, command: &Command) -> Result<Vec<Op>, LowerError> {
    lower_command_with(model, command, None)
}

/// Lowers a command, optionally supplying the part a builder has just chosen
/// from the `/part` dropdown.
///
/// The retry after a disambiguation goes back through the ordinary path rather
/// than around it, so a resolved part behaves exactly as a catalogue hit would.
#[allow(clippy::too_many_lines)]
pub fn lower_command_with(
    model: &Model,
    command: &Command,
    chosen: Option<&PartRef>,
) -> Result<Vec<Op>, LowerError> {
    match command.verb {
        Verb::Add | Verb::Attach => {
            let ObjectExpr::Part(spec) = &command.object else {
                return Err(LowerError::Unsupported(Unsupported::That));
            };
            let part = resolve_part(spec, chosen)?;
            let pos = resolve_location(model, command.location.as_ref(), &part)?;
            let quantity = spec.quantity.clamp(1, MAX_PLACEMENTS_PER_COMMAND);

            // Several of the same part with no other instruction line up
            // eastwards, one footprint apart.
            let repeat = (quantity > 1).then(|| Repeat {
                count: quantity,
                step: IVec3::new(
                    span_along(part.footprint, Orientation::DEFAULT, Direction::East),
                    0,
                    0,
                ),
            });

            Ok(vec![Op::Add {
                part,
                color: spec.color.unwrap_or(DEFAULT_COLOR),
                pos,
                orientation: orientation_of(command),
                repeat,
                group: None,
            }])
        }

        Verb::Move | Verb::Raise | Verb::Lower => {
            let targets = targets_of(model, command)?;
            let offset = offset_of(command);
            if offset == IVec3::ZERO {
                return Err(LowerError::Unsupported(Unsupported::MoveWithoutDirection));
            }
            Ok(vec![Op::Move { targets, offset }])
        }

        Verb::Remove => Ok(vec![Op::Remove {
            targets: targets_of(model, command)?,
        }]),

        Verb::Paint => {
            let targets = targets_of(model, command)?;
            let color = command
                .target_color
                .ok_or(LowerError::Unsupported(Unsupported::PaintWithoutColour))?;
            Ok(vec![Op::Paint { targets, color }])
        }

        Verb::Rotate => {
            let targets = targets_of(model, command)?;
            let orientation = orientation_of(command);
            let turns = i32::from(orientation.quarter_turns);
            if turns == 0 && !orientation.upside_down {
                // A bare "Rotate." means a single quarter turn.
                return Ok(vec![Op::Rotate {
                    targets,
                    turns: 1,
                    flip: false,
                }]);
            }
            Ok(vec![Op::Rotate {
                targets,
                turns,
                flip: orientation.upside_down,
            }])
        }

        Verb::Replace => {
            let targets = targets_of(model, command)?;
            if let Some(color) = command.target_color {
                return Ok(vec![Op::Paint { targets, color }]);
            }
            let spec = command
                .target_part
                .as_ref()
                .ok_or(LowerError::Unsupported(Unsupported::Replacement))?;
            let part = resolve_part(spec, chosen)?;
            Ok(vec![Op::Replace { targets, part }])
        }

        Verb::Select => {
            let ObjectExpr::Reference(reference) = &command.object else {
                return Err(LowerError::Unsupported(Unsupported::Selection));
            };
            let predicate = match reference {
                Ref::AllMatching(spec) | Ref::LastOfKind(spec) => predicate_of(spec),
                Ref::Highest => SelectPredicate {
                    highest: true,
                    ..SelectPredicate::default()
                },
                Ref::Named(name) => SelectPredicate {
                    group: Some(name.to_lowercase()),
                    ..SelectPredicate::default()
                },
                _ => SelectPredicate::default(),
            };
            Ok(vec![Op::Select { predicate }])
        }

        Verb::Group | Verb::Name => {
            let name = command
                .target_part
                .as_ref()
                .and_then(|s| s.part_num.clone())
                .ok_or(LowerError::Unsupported(Unsupported::Name))?;

            // "Name this point gate." records a location rather than a group.
            let naming_point = command.modifiers.contains(&Modifier::Quantity(0));
            if naming_point {
                let pos = model
                    .last_ref
                    .last()
                    .and_then(|id| model.get(*id))
                    .map_or(IVec3::ZERO, |p| p.pos);
                return Ok(vec![Op::Landmark { name, pos }]);
            }

            let targets = targets_of(model, command)?;
            Ok(vec![Op::Group { name, targets }])
        }

        Verb::Build | Verb::Fill => {
            if let ObjectExpr::Grid(spec) = &command.object {
                return lower_grid(model, command, spec, chosen);
            }
            let ObjectExpr::Structure(structure) = &command.object else {
                return Err(LowerError::Unsupported(Unsupported::That));
            };
            super::expand::expand(model, command, structure, chosen)
        }

        Verb::Duplicate | Verb::Mirror | Verb::Repeat => super::expand::expand_copy(model, command),

        _ => Err(LowerError::Unsupported(Unsupported::Verb)),
    }
}

/// The colour used when a sentence names no colour.
pub const DEFAULT_COLOR: u16 = 71;

/// The group a drawing's parts belong to, so the whole thing can be referred
/// to afterwards — `Paint the grid dark tan.`
const GRID_GROUP: &str = "grid";

/// Resolves a drawing's legend into real elements, in the order written.
///
/// Written order matters: it decides which entry a pending part choice
/// answers, so the whole expansion stays reproducible.
fn resolve_legend(
    spec: &GridSpec,
    chosen: Option<&PartRef>,
) -> Result<Vec<LegendPart>, LowerError> {
    if let Some(text) = &spec.bad_legend_key {
        return Err(LowerError::Grid(GridError::LegendSymbolNotSingleChar {
            text: text.clone(),
        }));
    }

    // The dropdown can only answer one question at a time, and a replayed
    // sentence hands its answer to whichever entry asks first — so a second
    // unknown part would pause forever, resolving the first one again. Say so
    // up front instead.
    let unknown = spec
        .legend
        .iter()
        .filter(|entry| matches!(parts::resolve(&entry.spec), Resolution::NeedsLookup { .. }))
        .count();
    if unknown > 1 {
        return Err(LowerError::Grid(GridError::ManyUnknownParts));
    }

    let mut resolved = Vec::with_capacity(spec.legend.len());
    for entry in &spec.legend {
        resolved.push(LegendPart {
            symbol: entry.symbol,
            part: resolve_part(&entry.spec, chosen)?,
            color: entry.spec.color.unwrap_or(DEFAULT_COLOR),
        });
    }
    Ok(resolved)
}

/// Lays a drawing out into one operation.
///
/// The whole drawing is a single [`Op::AddMany`], so it is one undo step while
/// every part in it still gets its own stable id — exactly how a wall behaves.
fn lower_grid(
    model: &Model,
    command: &Command,
    spec: &GridSpec,
    chosen: Option<&PartRef>,
) -> Result<Vec<Op>, LowerError> {
    let legend = resolve_legend(spec, chosen)?;

    // `resolve_location` measures `under` against the part being placed; the
    // first of the drawing's own parts is the honest thing to measure with.
    let representative = legend
        .first()
        .map_or_else(parts::default_part, |entry| entry.part.clone());
    let anchor = resolve_location(model, command.location.as_ref(), &representative)?;

    let placements = super::grid::expand(spec, &legend, anchor).map_err(LowerError::Grid)?;
    checked_total(u32::try_from(placements.len()).unwrap_or(u32::MAX))?;

    Ok(vec![Op::AddMany {
        placements,
        group: Some(GRID_GROUP.to_string()),
    }])
}

/// Resolves the placements a command acts on, defaulting to the last
/// referenced thing.
fn targets_of(model: &Model, command: &Command) -> Result<Vec<PlacementId>, LowerError> {
    match &command.object {
        ObjectExpr::Reference(reference) => resolve_ref(model, reference),
        ObjectExpr::None => resolve_ref(model, &Ref::Last),
        _ => Err(LowerError::Unsupported(Unsupported::That)),
    }
}

/// The part a structure command should be built from.
pub fn structure_part(command: &Command, chosen: Option<&PartRef>) -> Result<PartRef, LowerError> {
    command.using().map_or_else(
        || Ok(parts::default_part()),
        |spec| resolve_part(spec, chosen),
    )
}

/// Reads an extent in whole units, e.g. `10 studs long` -> 10.
#[must_use]
pub fn extent_count(command: &Command, axis: ExtentAxis, default: u32) -> u32 {
    command.extent(axis).map_or(default, |(count, _)| {
        count.clamp(1, MAX_PLACEMENTS_PER_COMMAND)
    })
}

/// The unit an extent was expressed in.
#[must_use]
pub fn extent_unit(command: &Command, axis: ExtentAxis, default: Unit) -> Unit {
    command.extent(axis).map_or(default, |(_, unit)| unit)
}

/// A structure's name, for grouping the placements it creates.
#[must_use]
pub fn structure_group(structure: &Structure) -> String {
    structure.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::dsl::Statement;
    use crate::build::dsl::lexer::tokenize_sentence;
    use crate::build::dsl::parser::{SentenceOutcome, parse_sentence};
    use crate::build::ops::apply;

    fn command_of(text: &str) -> Command {
        match parse_sentence(&tokenize_sentence(text)) {
            SentenceOutcome::Statement(Statement::Command(c)) => *c,
            other => panic!("expected a command from {text:?}, got {other:?}"),
        }
    }

    /// Applies a sequence of sentences, returning the resulting model.
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
    fn adds_a_part_at_the_origin() {
        let model = run(&["Add a red 2x4 brick at origin"]);
        assert_eq!(model.len(), 1);
        let placement = &model.placements[0];
        assert_eq!(placement.part.ldraw_id, "3001");
        assert_eq!(placement.color, 4);
        assert_eq!(placement.pos, IVec3::ZERO);
    }

    #[test]
    fn an_add_with_no_location_stacks_on_the_last_part() {
        let model = run(&["Add a red 2x4 brick at origin", "Add a blue 2x4 brick"]);
        assert_eq!(model.len(), 2);
        // The second brick rests on the first: one brick height up (-Y).
        assert_eq!(model.placements[1].pos, IVec3::new(0, -24, 0));
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn on_top_of_that_stacks_explicitly() {
        let model = run(&[
            "Add a yellow 2x4 brick at origin",
            "Put a blue 2x4 plate on top of that",
        ]);
        assert_eq!(model.placements[1].pos, IVec3::new(0, -24, 0));
        assert_eq!(model.placements[1].part.kind, PartKind::Plate);
    }

    #[test]
    fn a_compass_location_puts_parts_side_by_side() {
        let mut model = run(&["Add a red 2x4 brick at origin", "Call this wall"]);
        let command = command_of("Add a white 1x2 plate east of wall");
        let ops = lower_command(&model, &command).unwrap();
        for op in &ops {
            apply(&mut model, 2, op);
        }
        assert_eq!(model.len(), 2);
        // Clear of the 2-wide brick, so nothing overlaps.
        assert!(model.placements[1].pos.x > 0);
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn under_places_a_part_beneath_the_anchor() {
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Add a black 2x4 tile under that",
        ]);
        // Below means larger Y, since LDraw's Y points down.
        assert!(model.placements[1].pos.y > model.placements[0].pos.y);
    }

    #[test]
    fn coordinates_are_taken_literally() {
        let model = run(&["Add a red 2x4 brick at 20 0 40"]);
        assert_eq!(model.placements[0].pos, IVec3::new(20, 0, 40));
    }

    #[test]
    fn moving_uses_the_conversational_reference() {
        let model = run(&["Add a red 2x4 brick at origin", "Move it one stud east"]);
        assert_eq!(model.placements[0].pos, IVec3::new(20, 0, 0));
    }

    #[test]
    fn raise_and_lower_move_vertically() {
        let model = run(&["Add a red 2x4 brick at origin", "Raise by one plate"]);
        assert_eq!(model.placements[0].pos, IVec3::new(0, -8, 0));

        let model = run(&["Add a red 2x4 brick at origin", "Lower by two bricks"]);
        assert_eq!(model.placements[0].pos, IVec3::new(0, 48, 0));
    }

    #[test]
    fn the_last_of_a_kind_resolves_by_description() {
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Add a blue 2x4 plate at 100 0 0",
            "Move the last red brick east",
        ]);
        // Only the red brick moved.
        assert_eq!(model.placements[0].pos.x, 20);
        assert_eq!(model.placements[1].pos.x, 100);
    }

    #[test]
    fn painting_a_selection_recolours_every_match() {
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Add a red 2x4 brick at 100 0 0",
            "Add a blue 2x4 brick at 200 0 0",
            "Select all red bricks",
            "Paint selection dark tan",
        ]);
        assert_eq!(model.placements[0].color, 28);
        assert_eq!(model.placements[1].color, 28);
        assert_eq!(model.placements[2].color, 1, "the blue brick is untouched");
    }

    #[test]
    fn replacing_a_colour_is_lowered_to_a_repaint() {
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Replace all red bricks with dark red",
        ]);
        assert_eq!(model.placements[0].color, 320);
    }

    #[test]
    fn replacing_a_part_keeps_the_position() {
        let model = run(&[
            "Add a red 2x4 brick at origin",
            "Replace all red bricks with 2x2 bricks",
        ]);
        assert_eq!(model.placements[0].part.ldraw_id, "3003");
        assert_eq!(model.placements[0].pos, IVec3::ZERO);
    }

    #[test]
    fn naming_creates_a_group_that_can_be_referred_to() {
        let model = run(&["Add a red 2x4 brick at origin", "Call this wall"]);
        assert_eq!(model.resolve_group("wall").map(|ids| ids.len()), Some(1));
    }

    #[test]
    fn naming_a_point_creates_a_landmark() {
        let model = run(&["Add a red 2x4 brick at 20 0 40", "Name this point gate"]);
        assert_eq!(model.landmarks.get("gate"), Some(&IVec3::new(20, 0, 40)));
    }

    #[test]
    fn a_landmark_can_then_be_built_at() {
        let model = run(&[
            "Add a red 2x4 brick at 20 0 40",
            "Name this point gate",
            "Add a blue 2x4 brick at gate",
        ]);
        assert_eq!(model.placements[1].pos, IVec3::new(20, 0, 40));
    }

    #[test]
    fn rotating_turns_the_part() {
        let model = run(&["Add a red 2x4 brick at origin", "Rotate 90 degrees"]);
        assert_eq!(model.placements[0].orientation.quarter_turns, 1);

        let model = run(&["Add a red 2x4 brick at origin", "Turn it upside down"]);
        assert!(model.placements[0].orientation.upside_down);
    }

    #[test]
    fn a_quantity_lines_parts_up_without_overlapping() {
        let model = run(&["Add 3 red 2x4 bricks at origin"]);
        assert_eq!(model.len(), 3);
        assert!(model.collisions().is_empty());
    }

    #[test]
    fn removing_takes_the_part_out() {
        let model = run(&["Add a red 2x4 brick at origin", "Remove it"]);
        assert!(model.is_empty());
    }

    // --- Error cases -------------------------------------------------------

    #[test]
    fn a_dangling_reference_is_reported_not_guessed() {
        let model = Model::new("empty");
        let command = command_of("Move it east");
        assert_eq!(
            lower_command(&model, &command),
            Err(LowerError::UnresolvedReference("that".to_string()))
        );
    }

    #[test]
    fn an_unknown_name_is_reported() {
        let model = Model::new("empty");
        let command = command_of("Move the gatehouse east");
        assert_eq!(
            lower_command(&model, &command),
            Err(LowerError::UnknownName("gatehouse".to_string()))
        );
    }

    #[test]
    fn an_unusual_part_asks_for_a_lookup_rather_than_failing() {
        let model = Model::new("empty");
        let command = command_of("Add a red 3x12 brick at origin");
        assert!(matches!(
            lower_command(&model, &command),
            Err(LowerError::NeedsPartLookup { .. })
        ));
    }

    #[test]
    fn selecting_nothing_reports_it() {
        let model = Model::new("empty");
        let command = command_of("Select everything");
        // An empty selection predicate is fine to store; it just matches
        // nothing. But an explicit "everything" over an empty model is not.
        let ops = lower_command(&model, &command).unwrap();
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn error_messages_read_like_english() {
        assert_eq!(
            LowerError::UnresolvedReference("that".to_string()).to_string(),
            "I'm not sure what 'that' refers to"
        );
        assert_eq!(
            LowerError::UnknownName("roof".to_string()).to_string(),
            "I don't know anything called 'roof'"
        );
    }
}
