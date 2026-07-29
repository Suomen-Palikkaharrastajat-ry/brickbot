//! The build language's abstract syntax tree.
//!
//! Mirrors the grammar sketch directly: a command is a verb, an object, an
//! optional location and any number of modifiers. Nothing here is resolved
//! against the model — `that`, `the wall` and `a red 2x4 brick` stay symbolic
//! until [`super::lower`] turns them into operations.

#![allow(clippy::doc_markdown)]

use super::lexer::Verb;
use crate::build::model::PartKind;
use crate::build::units::{Direction, IVec3, Unit};

/// A part as described in a sentence, before it has been resolved to a real
/// element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartSpec {
    pub color: Option<u16>,
    pub size: Option<(u8, u8)>,
    pub kind: Option<PartKind>,
    /// An explicit part number, when the builder gave one.
    pub part_num: Option<String>,
    /// How many to place. Defaults to one.
    pub quantity: u32,
}

impl PartSpec {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.color.is_none()
            && self.size.is_none()
            && self.kind.is_none()
            && self.part_num.is_none()
    }

    /// The search string to hand to the part lookup, e.g. `Brick 2 x 4`.
    #[must_use]
    pub fn search_query(&self) -> String {
        if let Some(part_num) = &self.part_num {
            return part_num.clone();
        }
        let kind = self.kind.map_or("brick", PartKind::as_str);
        self.size
            .map_or_else(|| kind.to_string(), |(w, d)| format!("{kind} {w} x {d}"))
    }
}

/// How a sentence refers to something already in the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// `that`, `it`
    Last,
    /// `them`, `those`
    LastPlural,
    /// `the last brick`, `the last red plate`
    LastOfKind(Box<PartSpec>),
    /// A named group or landmark.
    Named(String),
    /// `the selection`
    Selection,
    /// `everything`, `all`
    Everything,
    /// `all red bricks`
    AllMatching(Box<PartSpec>),
    /// `the highest brick`
    Highest,
}

/// A point in space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Origin,
    Coords(IVec3),
    /// A named landmark such as `gate`.
    Landmark(String),
}

/// Where an object goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    At(Place),
    /// `on the blue plate`, `on top of that`
    On(Ref),
    /// `under the roof`
    Under(Ref),
    /// `north of the window`
    Direction {
        direction: Direction,
        of: Ref,
    },
}

/// Anything that qualifies a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modifier {
    /// `3 times`
    Quantity(u32),
    /// Quarter turns; negative turns anticlockwise.
    Turn(i32),
    /// `upside down`
    Flip,
    Facing(Direction),
    /// `east 3 studs`
    Distance {
        direction: Direction,
        count: i32,
        unit: Unit,
    },
    /// `10 studs long`, `6 bricks high`
    Extent {
        axis: ExtentAxis,
        count: u32,
        unit: Unit,
    },
    /// `using tan plates`
    Using(Box<PartSpec>),
}

/// Which dimension an extent modifier measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtentAxis {
    Length,
    Height,
    Width,
    Depth,
}

/// What a command acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectExpr {
    /// A new part to place.
    Part(Box<PartSpec>),
    /// Something already in the model.
    Reference(Ref),
    /// A high-level structure to build.
    Structure(Structure),
    /// No object at all, e.g. `Undo.`
    None,
}

/// A structure a `build` verb can raise, expanded server-side into placements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Structure {
    Wall,
    Row,
    Floor,
    Tower,
    Box,
    Stairs,
    /// Something we do not know how to expand, kept so the bot can say so.
    Unknown(String),
}

impl Structure {
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "wall" => Self::Wall,
            "row" | "line" | "course" => Self::Row,
            "floor" | "base" | "baseplate" => Self::Floor,
            "tower" | "column" | "pillar" => Self::Tower,
            "box" | "enclosure" | "cube" => Self::Box,
            "stairs" | "staircase" | "steps" => Self::Stairs,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Wall => "wall",
            Self::Row => "row",
            Self::Floor => "floor",
            Self::Tower => "tower",
            Self::Box => "box",
            Self::Stairs => "stairs",
            Self::Unknown(_) => "structure",
        }
    }
}

/// A complete imperative command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub verb: Verb,
    pub object: ObjectExpr,
    pub location: Option<Location>,
    pub modifiers: Vec<Modifier>,
    /// For `Paint ... dark tan` and `Replace ... with X`.
    pub target_color: Option<u16>,
    pub target_part: Option<Box<PartSpec>>,
}

impl Command {
    #[must_use]
    pub fn distance(&self) -> Option<(Direction, i32, Unit)> {
        self.modifiers.iter().find_map(|m| match m {
            Modifier::Distance {
                direction,
                count,
                unit,
            } => Some((*direction, *count, *unit)),
            _ => None,
        })
    }

    #[must_use]
    pub fn extent(&self, axis: ExtentAxis) -> Option<(u32, Unit)> {
        self.modifiers.iter().find_map(|m| match m {
            Modifier::Extent {
                axis: a,
                count,
                unit,
            } if *a == axis => Some((*count, *unit)),
            _ => None,
        })
    }

    #[must_use]
    pub fn quantity(&self) -> Option<u32> {
        self.modifiers.iter().find_map(|m| match m {
            Modifier::Quantity(n) => Some(*n),
            _ => None,
        })
    }

    #[must_use]
    pub fn using(&self) -> Option<&PartSpec> {
        self.modifiers.iter().find_map(|m| match m {
            Modifier::Using(spec) => Some(spec.as_ref()),
            _ => None,
        })
    }
}

/// A question about the model, which changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    HowTall(Option<Ref>),
    Count(Box<PartSpec>),
    FindCollisions,
    ShowUnsupported,
    WhereIs(Ref),
    WhoAdded(Ref),
    WhatChanged,
    ListParts,
}

/// A session-level command that is not an edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Meta {
    Undo,
    /// `Undo Alice's last edit.`
    UndoBy(String),
    Redo,
    RevertTo(u32),
    Export,
    Render,
    Help,
    Finish,
}

/// One parsed sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Command(Box<Command>),
    Query(Query),
    Meta(Meta),
    /// `Note:` / `TODO:` — acknowledged but never acted on.
    Comment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_part_spec_is_recognised() {
        assert!(PartSpec::default().is_empty());
        assert!(
            !PartSpec {
                color: Some(4),
                ..PartSpec::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn part_search_queries_read_like_catalogue_names() {
        let spec = PartSpec {
            size: Some((2, 4)),
            kind: Some(PartKind::Brick),
            ..PartSpec::default()
        };
        assert_eq!(spec.search_query(), "brick 2 x 4");

        let plate = PartSpec {
            size: Some((1, 2)),
            kind: Some(PartKind::Plate),
            ..PartSpec::default()
        };
        assert_eq!(plate.search_query(), "plate 1 x 2");
    }

    #[test]
    fn an_explicit_part_number_wins_over_the_description() {
        let spec = PartSpec {
            part_num: Some("3001".to_string()),
            size: Some((2, 4)),
            kind: Some(PartKind::Brick),
            ..PartSpec::default()
        };
        assert_eq!(spec.search_query(), "3001");
    }

    #[test]
    fn a_bare_kind_still_produces_a_query() {
        let spec = PartSpec {
            kind: Some(PartKind::Tile),
            ..PartSpec::default()
        };
        assert_eq!(spec.search_query(), "tile");
        assert_eq!(PartSpec::default().search_query(), "brick");
    }

    #[test]
    fn structures_parse_from_their_common_names() {
        assert_eq!(Structure::parse("wall"), Some(Structure::Wall));
        assert_eq!(Structure::parse("column"), Some(Structure::Tower));
        assert_eq!(Structure::parse("staircase"), Some(Structure::Stairs));
        assert_eq!(Structure::parse("gazebo"), None);
        assert_eq!(Structure::Wall.as_str(), "wall");
    }

    #[test]
    fn modifier_accessors_find_the_right_modifier() {
        let command = Command {
            verb: Verb::Build,
            object: ObjectExpr::Structure(Structure::Wall),
            location: None,
            modifiers: vec![
                Modifier::Extent {
                    axis: ExtentAxis::Length,
                    count: 10,
                    unit: Unit::Stud,
                },
                Modifier::Extent {
                    axis: ExtentAxis::Height,
                    count: 6,
                    unit: Unit::Brick,
                },
                Modifier::Quantity(3),
            ],
            target_color: None,
            target_part: None,
        };

        assert_eq!(command.extent(ExtentAxis::Length), Some((10, Unit::Stud)));
        assert_eq!(command.extent(ExtentAxis::Height), Some((6, Unit::Brick)));
        assert_eq!(command.extent(ExtentAxis::Width), None);
        assert_eq!(command.quantity(), Some(3));
        assert_eq!(command.distance(), None);
        assert!(command.using().is_none());
    }
}
