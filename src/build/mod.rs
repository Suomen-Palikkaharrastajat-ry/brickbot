//! Collaborative LEGO building: the model, the build language, and the
//! session that ties them to a Discord thread.
//!
//! The layering mirrors the rest of the crate's conventions: everything except
//! `session` is pure and unit-testable, with no serenity, reqwest or sqlx
//! types crossing into it.

pub mod model;
pub mod ops;
pub mod units;

pub use model::{Model, PartKind, PartRef, Placement, PlacementId};
pub use ops::{History, Op, SelectPredicate};
pub use units::{Direction, IVec3, Orientation, Unit, LDU_BRICK, LDU_PLATE, LDU_STUD};
