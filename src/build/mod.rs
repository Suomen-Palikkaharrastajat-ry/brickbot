//! Collaborative LEGO building: the model, the build language, and the
//! session that ties them to a Discord thread.
//!
//! The layering mirrors the rest of the crate's conventions: everything except
//! `session` is pure and unit-testable, with no serenity, reqwest or sqlx
//! types crossing into it.

pub mod debug_log;
pub mod dispatch;
pub mod draft;
pub mod dsl;
pub mod expand;
pub mod export;
pub mod grid;
pub mod legal;
pub mod lower;
pub mod model;
pub mod ops;
pub mod parts;
pub mod propose;
pub mod queries;
pub mod report;
pub mod scene;
pub mod session;
pub mod textview;
pub mod units;

pub use lower::{LowerError, lower_command};
pub use model::{Model, PartKind, PartRef, Placement, PlacementId};
pub use ops::{History, Op, SelectPredicate};
pub use units::{Direction, IVec3, LDU_BRICK, LDU_PLATE, LDU_STUD, Orientation, Unit};
