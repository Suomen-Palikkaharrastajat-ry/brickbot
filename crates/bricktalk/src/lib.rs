//! `BrickTalk`: the controlled natural language behind `/build`, and everything
//! that interprets it — parsing, lowering to model operations, legal-geometry
//! correction, placement proposals, text views and the localized replies.
//!
//! No Discord, database or network types cross into this crate, and nothing in
//! it calls an LLM: interpretation is deterministic and reproducible.

rust_i18n::i18n!("../../locales", fallback = "en-US");

pub mod discord_limits;
pub mod dispatch;
pub mod draft;
pub mod dsl;
pub mod expand;
pub mod grid;
pub mod legal;
pub mod lower;
pub mod parts;
pub mod propose;
pub mod queries;
pub mod report;
pub mod textview;

// The lower layers, under the paths the modules above were written against.
pub use brick_model::{export, model, ops, units};
pub use brick_render::{ldraw, math, render};

pub use lower::{LowerError, lower_command};
pub use model::{Model, PartKind, PartRef, Placement, PlacementId};
pub use ops::{History, Op, SelectPredicate};
pub use units::{Direction, IVec3, LDU_BRICK, LDU_PLATE, LDU_STUD, Orientation, Unit};
