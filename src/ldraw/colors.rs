//! The LDraw colour palette.
//!
//! Two directions are needed:
//!
//! * code -> RGB, for the renderer and for writing `.ldr` files;
//! * name -> code, so the build language can understand `a red 2x4 brick`.
//!
//! The table covers the solid and transparent colours LDraw defines for real
//! LEGO elements. It is a curated subset of `LDConfig.ldr` rather than a parse
//! of it, so that colour lookup never needs the network.

#![allow(clippy::doc_markdown, clippy::many_single_char_names)]

use super::types::{COLOR_EDGE, COLOR_INHERIT};

/// An 8-bit-per-channel colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// One palette entry: the LDraw code, its canonical name, and its colour.
struct Entry {
    code: u16,
    name: &'static str,
    rgb: Rgb,
}

const fn e(code: u16, name: &'static str, r: u8, g: u8, b: u8) -> Entry {
    Entry {
        code,
        name,
        rgb: Rgb::new(r, g, b),
    }
}

/// The fallback for any code we do not know, so an unknown colour renders as
/// a neutral part rather than disappearing.
const UNKNOWN: Rgb = Rgb::new(0x9B, 0xA1, 0x9D);

#[rustfmt::skip]
static PALETTE: &[Entry] = &[
    e(0,   "black",                0x05, 0x13, 0x1D),
    e(1,   "blue",                 0x00, 0x55, 0xBF),
    e(2,   "green",                0x23, 0x78, 0x41),
    e(3,   "dark turquoise",       0x00, 0x8F, 0x9B),
    e(4,   "red",                  0xC9, 0x1A, 0x09),
    e(5,   "dark pink",            0xC8, 0x70, 0xA0),
    e(6,   "brown",                0x58, 0x39, 0x27),
    // LDraw calls 7 and 8 "Light_Grey"/"Dark_Grey", but LEGO retired both in
    // 2004 in favour of the bluish greys. The plain names are therefore given
    // to 71/72 via the alias table, and these keep a qualified name so that
    // code -> name -> code still round-trips.
    e(7,   "legacy light grey",    0x9B, 0xA1, 0x9D),
    e(8,   "legacy dark grey",     0x6D, 0x6E, 0x5C),
    e(9,   "light blue",           0xB4, 0xD2, 0xE3),
    e(10,  "bright green",         0x4B, 0x9F, 0x4A),
    e(11,  "light turquoise",      0x55, 0xA5, 0xAF),
    e(12,  "salmon",               0xF2, 0x70, 0x5E),
    e(13,  "pink",                 0xFC, 0x97, 0xAC),
    e(14,  "yellow",               0xF2, 0xCD, 0x37),
    e(15,  "white",                0xFF, 0xFF, 0xFF),
    e(17,  "light green",          0xC2, 0xDA, 0xB8),
    e(18,  "light yellow",         0xFB, 0xE6, 0x96),
    e(19,  "tan",                  0xE4, 0xCD, 0x9E),
    e(20,  "light violet",         0xC9, 0xCA, 0xE2),
    e(22,  "purple",               0x81, 0x00, 0x7B),
    e(23,  "dark blue violet",     0x20, 0x32, 0xB0),
    e(25,  "orange",               0xFE, 0x8A, 0x18),
    e(26,  "magenta",              0x92, 0x39, 0x78),
    e(27,  "lime",                 0xBB, 0xE9, 0x0B),
    e(28,  "dark tan",             0x95, 0x8A, 0x73),
    e(29,  "bright pink",          0xE4, 0xAD, 0xC8),
    e(30,  "medium lavender",      0xAC, 0x78, 0xBA),
    e(31,  "lavender",             0xE1, 0xD5, 0xED),
    e(68,  "very light orange",    0xF3, 0xCF, 0x9B),
    e(69,  "light purple",         0xCD, 0x62, 0x98),
    e(70,  "reddish brown",        0x58, 0x2A, 0x12),
    e(71,  "light bluish grey",    0xA0, 0xA5, 0xA9),
    e(72,  "dark bluish grey",     0x6C, 0x6E, 0x68),
    e(73,  "medium blue",          0x5A, 0x93, 0xDB),
    e(74,  "medium green",         0x73, 0xDC, 0xA1),
    e(77,  "light pink",           0xFE, 0xCC, 0xCF),
    e(78,  "light nougat",         0xF6, 0xD7, 0xB3),
    e(84,  "medium nougat",        0xCC, 0x70, 0x2A),
    e(85,  "dark purple",          0x3F, 0x36, 0x91),
    e(272, "dark blue",            0x0A, 0x34, 0x63),
    e(288, "dark green",           0x18, 0x46, 0x32),
    e(320, "dark red",             0x72, 0x0E, 0x0F),
    e(321, "dark azure",           0x14, 0x98, 0xD7),
    e(322, "medium azure",         0x3E, 0xC2, 0xDD),
    e(323, "light aqua",           0xBD, 0xDC, 0xD8),
    e(326, "yellowish green",      0xDF, 0xEE, 0xA5),
    e(335, "sand red",             0xD6, 0x75, 0x72),
    e(351, "medium dark pink",     0xF7, 0x85, 0xB1),
    e(366, "earth orange",         0xFA, 0x9C, 0x1C),
    e(373, "sand purple",          0x75, 0x65, 0x7D),
    e(378, "sand green",           0x70, 0x8E, 0x7C),
    e(379, "sand blue",            0x70, 0x81, 0x9A),
    e(450, "fabuland brown",       0xD2, 0x77, 0x44),
    e(462, "medium orange",        0xF5, 0x86, 0x24),
    e(484, "dark orange",          0x91, 0x50, 0x1C),
    e(503, "very light grey",      0xBC, 0xB4, 0xA5),
    // Transparent colours. Rendered opaque for now; the alpha channel is not
    // part of the framebuffer.
    e(33,  "trans dark blue",      0x00, 0x20, 0xA0),
    e(34,  "trans green",          0x23, 0x78, 0x41),
    e(35,  "trans bright green",   0x56, 0xE6, 0x46),
    e(36,  "trans red",            0xC9, 0x1A, 0x09),
    e(37,  "trans dark pink",      0xDF, 0x66, 0x95),
    e(38,  "trans neon orange",    0xFF, 0x80, 0x0D),
    e(39,  "trans very light blue",0xC1, 0xDF, 0xF0),
    e(40,  "trans black",          0x63, 0x5F, 0x52),
    e(41,  "trans medium blue",    0x55, 0x9A, 0xB7),
    e(42,  "trans neon green",     0xC0, 0xFF, 0x00),
    e(43,  "trans light blue",     0xAE, 0xE9, 0xEF),
    e(45,  "trans pink",           0xFC, 0x97, 0xAC),
    e(46,  "trans yellow",         0xF5, 0xCD, 0x2F),
    e(47,  "trans clear",          0xFC, 0xFC, 0xFC),
    e(52,  "trans purple",         0xA5, 0xA5, 0xCB),
    e(54,  "trans neon yellow",    0xDA, 0xB0, 0x00),
    e(57,  "trans orange",         0xF0, 0x8F, 0x1C),
];

/// Aliases accepted by the build language on top of the canonical names.
///
/// A bare `grey` resolves to Light Bluish Grey (71) rather than the legacy
/// Light Grey (7), because that is the grey LEGO has actually used since 2004.
#[rustfmt::skip]
static ALIASES: &[(&str, u16)] = &[
    ("grey",                71),
    ("light grey",          71),
    ("dark grey",           72),
    ("old light grey",      7),
    ("old dark grey",       8),
    ("silver",              71),
    ("nougat",              78),
    ("flesh",               78),
    ("clear",               47),
    ("transparent",         47),
    ("azure",               321),
    ("turquoise",           3),
    ("violet",              20),
    ("gold",                14),
];

/// Resolves an LDraw colour code to an RGB triple.
///
/// [`COLOR_INHERIT`] and [`COLOR_EDGE`] have no colour of their own; callers
/// must substitute the instantiating colour before calling this. They resolve
/// to a neutral grey rather than panicking.
#[must_use]
pub fn resolve(code: u16) -> Rgb {
    PALETTE
        .iter()
        .find(|entry| entry.code == code)
        .map_or(UNKNOWN, |entry| entry.rgb)
}

/// The canonical LDraw name of a colour code, if known.
#[must_use]
pub fn name_of(code: u16) -> Option<&'static str> {
    PALETTE
        .iter()
        .find(|entry| entry.code == code)
        .map(|entry| entry.name)
}

/// Normalises a colour phrase for lookup: lowercase, collapsed whitespace, and
/// American spellings folded onto the LDraw British ones.
fn normalize(name: &str) -> String {
    name.split_whitespace()
        .map(|word| match word.to_lowercase().as_str() {
            "gray" => "grey".to_string(),
            "bluish-gray" | "bluish-grey" => "bluish grey".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Looks up a colour code from a human colour phrase such as `dark tan`.
#[must_use]
pub fn code_from_name(name: &str) -> Option<u16> {
    let normalized = normalize(name);
    if normalized.is_empty() {
        return None;
    }
    PALETTE
        .iter()
        .find(|entry| entry.name == normalized)
        .map(|entry| entry.code)
        .or_else(|| {
            ALIASES
                .iter()
                .find(|(alias, _)| normalize(alias) == normalized)
                .map(|(_, code)| *code)
        })
}

/// True when the phrase names a colour. Used by the tokenizer to decide
/// whether to keep consuming words into a multi-word colour.
#[must_use]
pub fn is_color_name(name: &str) -> bool {
    code_from_name(name).is_some()
}

/// True when any known colour name starts with this phrase, which lets the
/// tokenizer greedily extend `dark` into `dark tan`.
#[must_use]
pub fn is_color_prefix(prefix: &str) -> bool {
    let normalized = normalize(prefix);
    if normalized.is_empty() {
        return false;
    }
    let with_space = format!("{normalized} ");
    PALETTE
        .iter()
        .any(|entry| entry.name.starts_with(&with_space))
        || ALIASES
            .iter()
            .any(|(alias, _)| normalize(alias).starts_with(&with_space))
}

/// Substitutes the placement colour for the `inherit` and `edge` codes.
#[must_use]
pub const fn resolve_inherited(code: u16, parent: u16) -> u16 {
    if code == COLOR_INHERIT || code == COLOR_EDGE {
        parent
    } else {
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_well_known_codes() {
        assert_eq!(resolve(4), Rgb::new(0xC9, 0x1A, 0x09));
        assert_eq!(resolve(15), Rgb::new(0xFF, 0xFF, 0xFF));
        assert_eq!(resolve(71), Rgb::new(0xA0, 0xA5, 0xA9));
    }

    #[test]
    fn unknown_codes_fall_back_to_neutral_grey() {
        assert_eq!(resolve(60000), UNKNOWN);
    }

    #[test]
    fn looks_up_codes_from_names() {
        assert_eq!(code_from_name("red"), Some(4));
        assert_eq!(code_from_name("Dark Tan"), Some(28));
        assert_eq!(code_from_name("  YELLOW  "), Some(14));
        assert_eq!(code_from_name("not a colour"), None);
        assert_eq!(code_from_name(""), None);
    }

    #[test]
    fn american_spellings_and_aliases_resolve() {
        assert_eq!(code_from_name("light bluish gray"), Some(71));
        assert_eq!(code_from_name("light bluish grey"), Some(71));
        assert_eq!(code_from_name("gray"), Some(71));
        assert_eq!(code_from_name("grey"), Some(71));
        assert_eq!(code_from_name("dark gray"), Some(72));
        // The retired 1990s greys stay reachable, but only when asked for.
        assert_eq!(code_from_name("old light grey"), Some(7));
        assert_eq!(code_from_name("legacy dark grey"), Some(8));
    }

    #[test]
    fn name_of_round_trips_through_code_from_name() {
        for entry in PALETTE {
            let name = name_of(entry.code).unwrap();
            assert_eq!(code_from_name(name), Some(entry.code), "failed for {name}");
        }
    }

    #[test]
    fn prefixes_are_detected_for_multi_word_colours() {
        assert!(is_color_prefix("dark"));
        assert!(is_color_prefix("light bluish"));
        assert!(!is_color_prefix("red"));
        assert!(!is_color_prefix(""));
        assert!(is_color_name("red"));
        assert!(!is_color_name("dark"));
    }

    #[test]
    fn inherit_and_edge_take_the_parent_colour() {
        assert_eq!(resolve_inherited(COLOR_INHERIT, 4), 4);
        assert_eq!(resolve_inherited(COLOR_EDGE, 4), 4);
        assert_eq!(resolve_inherited(14, 4), 14);
    }

    #[test]
    fn palette_has_no_duplicate_codes_or_names() {
        for (i, entry) in PALETTE.iter().enumerate() {
            for other in PALETTE.iter().skip(i + 1) {
                assert_ne!(entry.code, other.code, "duplicate code {}", entry.code);
                assert_ne!(entry.name, other.name, "duplicate name {}", entry.name);
            }
        }
    }
}
