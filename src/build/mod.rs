//! Collaborative LEGO building: the model, the build language, and the
//! session that ties them to a Discord thread.
//!
//! The layering mirrors the rest of the crate's conventions: everything except
//! `session` is pure and unit-testable, with no serenity, reqwest or sqlx
//! types crossing into it.

pub mod debug_log;
pub mod scene;
pub mod session;

pub use brick_model::{export, model, ops, units};
pub use bricktalk::{
    dispatch, draft, dsl, expand, grid, legal, lower, parts, propose, queries, report, textview,
};
pub use lower::{LowerError, lower_command};
pub use model::{Model, PartKind, PartRef, Placement, PlacementId};
pub use ops::{History, Op, SelectPredicate};
pub use units::{Direction, IVec3, LDU_BRICK, LDU_PLATE, LDU_STUD, Orientation, Unit};
