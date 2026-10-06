//! LDraw part-library support: parsing `.dat`/`.ldr` text, resolving the
//! colour palette, and flattening a part's sub-file tree into a mesh.
//!
//! Everything here is pure except [`resolve`], which is the single point that
//! reads from the library directory on disk (installed by the bot's `--fetch-ldraw`).

#![allow(clippy::doc_markdown, clippy::many_single_char_names)]

pub mod colors;
pub mod geometry;
pub mod library;
pub mod parser;
pub mod resolve;
pub mod types;

pub use types::{COLOR_EDGE, COLOR_INHERIT, LdrawLine, Mesh, MeshTriangle, SubfileRef};
