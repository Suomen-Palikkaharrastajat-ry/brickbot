//! Pure geometry and rendering for the `/build` command: `LDraw` parsing, the
//! part-library installer and resolver, a small software rasteriser and PNG
//! output. Nothing here knows about Discord or the bot's database or config.

pub mod ldraw;
pub mod math;
pub mod render;
