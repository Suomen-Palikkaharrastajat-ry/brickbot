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
    e(86,  "medium brown",         0x7B, 0x5D, 0x41),
    e(89,  "blue violet",          0x1C, 0x58, 0xA7),
    // Plain "Nougat": the skin-tone brown LEGO used to call "Flesh", distinct
    // from the lighter/pinker Light Nougat (78) above.
    e(92,  "nougat",               0xBB, 0x80, 0x5A),
    e(100, "light salmon",         0xF9, 0xB7, 0xA5),
    // Plain "Violet", distinct from the paler Light Violet (20) above.
    e(110, "violet",               0x26, 0x46, 0x9A),
    e(112, "medium violet",        0x48, 0x61, 0xAC),
    e(115, "medium lime",          0xB7, 0xD4, 0x25),
    e(118, "aqua",                 0x9C, 0xD6, 0xCC),
    e(120, "light lime",           0xDE, 0xEA, 0x92),
    e(125, "light orange",         0xF9, 0xA7, 0x77),
    e(128, "dark nougat",          0xAD, 0x61, 0x40),
    e(151, "very light bluish grey", 0xC8, 0xC8, 0xC8),
    e(191, "bright light orange",  0xFC, 0xAC, 0x00),
    e(212, "bright light blue",    0x9D, 0xC3, 0xF7),
    e(216, "rust",                 0x87, 0x2B, 0x17),
    e(218, "reddish lilac",        0x8E, 0x55, 0x97),
    e(219, "lilac",                0x56, 0x4E, 0x9D),
    e(226, "bright light yellow",  0xFF, 0xEC, 0x6C),
    e(232, "sky blue",             0x77, 0xC9, 0xD8),
    e(272, "dark blue",            0x0A, 0x34, 0x63),
    e(288, "dark green",           0x18, 0x46, 0x32),
    e(295, "flamingo pink",        0xFF, 0x94, 0xC2),
    e(308, "dark brown",           0x35, 0x21, 0x00),
    e(313, "maersk blue",          0xAB, 0xD9, 0xFF),
    e(320, "dark red",             0x72, 0x0E, 0x0F),
    e(321, "dark azure",           0x14, 0x98, 0xD7),
    e(322, "medium azure",         0x3E, 0xC2, 0xDD),
    e(323, "light aqua",           0xBD, 0xDC, 0xD8),
    e(326, "yellowish green",      0xDF, 0xEE, 0xA5),
    e(330, "olive green",          0x77, 0x77, 0x4E),
    e(335, "sand red",             0xD6, 0x75, 0x72),
    e(351, "medium dark pink",     0xF7, 0x85, 0xB1),
    e(353, "coral",                0xFF, 0x6D, 0x77),
    e(366, "earth orange",         0xFA, 0x9C, 0x1C),
    e(373, "sand purple",          0x75, 0x65, 0x7D),
    e(378, "sand green",           0x70, 0x8E, 0x7C),
    e(379, "sand blue",            0x70, 0x81, 0x9A),
    e(450, "fabuland brown",       0xD2, 0x77, 0x44),
    e(462, "medium orange",        0xF5, 0x86, 0x24),
    e(484, "dark orange",          0x91, 0x50, 0x1C),
    e(503, "very light grey",      0xBC, 0xB4, 0xA5),
    e(507, "light orange brown",   0xFA, 0x9C, 0x1C),
    e(508, "fabuland red",         0xFF, 0x80, 0x14),
    e(509, "fabuland orange",      0xCF, 0x8A, 0x47),
    e(510, "fabuland pastel green",0x78, 0xFC, 0x78),
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
    e(44,  "trans bright reddish lilac", 0x96, 0x70, 0x9F),
    e(45,  "trans pink",           0xFC, 0x97, 0xAC),
    e(46,  "trans yellow",         0xF5, 0xCD, 0x2F),
    e(47,  "trans clear",          0xFC, 0xFC, 0xFC),
    e(52,  "trans purple",         0xA5, 0xA5, 0xCB),
    e(54,  "trans neon yellow",    0xDA, 0xB0, 0x00),
    e(57,  "trans orange",         0xF0, 0x8F, 0x1C),
    e(231, "trans bright light orange", 0xFC, 0xB7, 0x6D),
    e(234, "trans fire yellow",    0xFB, 0xE8, 0x90),
    e(284, "trans reddish lilac",  0xC2, 0x81, 0xA5),
    e(285, "trans light green",    0x7D, 0xC2, 0x91),
    e(293, "trans light blue violet", 0x6B, 0xAB, 0xE4),
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
    // LEGO's old name for what LDraw now calls plain Nougat (92) — not Light
    // Nougat (78), which "nougat" on its own resolves to directly now that
    // it is its own palette entry.
    ("flesh",               92),
    ("clear",               47),
    ("transparent",         47),
    ("azure",               321),
    ("turquoise",           3),
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

/// Every colour in the palette, as `(code, name)`.
///
/// For pickers that have to show the whole list — an autocomplete fallback or
/// a select menu — rather than answer one query.
pub fn all() -> impl Iterator<Item = (u16, &'static str)> {
    PALETTE.iter().map(|entry| (entry.code, entry.name))
}

/// How far apart two colour names may be and still be worth suggesting.
///
/// Proportional to what was typed, so a long phrase tolerates a couple of
/// slips while a three-letter word does not match half the palette.
fn distance_budget(needle: &str) -> usize {
    match needle.chars().count() {
        0..=3 => 1,
        4..=7 => 2,
        _ => 3,
    }
}

/// Colour names close to what was typed, best first.
///
/// Ranks exact match, then names starting with the phrase, then names
/// containing it, then near misses by edit distance — and alphabetically
/// within each tier, so the same input always suggests the same things.
///
/// Aliases are candidates too: `grey` and `clear` are what people actually
/// type, and both resolve through [`code_from_name`], so a suggestion is
/// always something that will work if typed back.
#[must_use]
pub fn suggest(input: &str, limit: usize) -> Vec<&'static str> {
    let needle = normalize(input);
    if needle.is_empty() {
        return PALETTE.iter().take(limit).map(|entry| entry.name).collect();
    }

    let budget = distance_budget(&needle);
    let candidates = PALETTE
        .iter()
        .map(|entry| entry.name)
        .chain(ALIASES.iter().map(|(alias, _)| *alias));

    // (tier, distance, name) sorts into exactly the order described above.
    let mut scored: Vec<(u8, usize, &'static str)> = candidates
        .filter_map(|name| {
            let distance = levenshtein::levenshtein(&needle, name);
            let tier = if name == needle {
                0
            } else if name.starts_with(&needle) {
                1
            } else if name.contains(&needle) {
                2
            } else if distance <= budget {
                3
            } else {
                return None;
            };
            Some((tier, distance, name))
        })
        .collect();

    scored.sort_unstable();
    scored.dedup_by_key(|(_, _, name)| *name);
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, name)| name)
        .collect()
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
    fn every_palette_colour_is_enumerable() {
        let listed: Vec<_> = all().collect();
        assert_eq!(listed.len(), PALETTE.len());
        assert!(listed.contains(&(4, "red")));
        // And every listed name resolves back, so a picker can only ever
        // offer something that works when typed.
        for (code, name) in listed {
            assert_eq!(code_from_name(name), Some(code), "{name} does not resolve");
        }
    }

    #[test]
    fn an_exact_colour_suggests_itself_first() {
        assert_eq!(suggest("red", 5).first(), Some(&"red"));
        assert_eq!(suggest("dark tan", 5).first(), Some(&"dark tan"));
        // American spelling and case fold through `normalize` first.
        assert_eq!(
            suggest("Light Bluish Gray", 5).first(),
            Some(&"light bluish grey")
        );
    }

    #[test]
    fn a_prefix_suggests_the_colours_that_extend_it() {
        let dark = suggest("dark", 10);
        assert!(dark.contains(&"dark tan"), "got: {dark:?}");
        assert!(dark.contains(&"dark red"), "got: {dark:?}");
        // Every one of them genuinely starts with what was typed.
        assert!(
            dark.iter().filter(|n| n.starts_with("dark")).count() >= 3,
            "got: {dark:?}"
        );
    }

    #[test]
    fn a_misspelling_suggests_the_colour_meant() {
        // The cases that actually come up.
        for (typo, expected) in [
            ("yelow", "yellow"),
            ("orage", "orange"),
            ("purpel", "purple"),
            ("blak", "black"),
        ] {
            let got = suggest(typo, 5);
            assert!(
                got.contains(&expected),
                "{typo:?} should have suggested {expected:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn aliases_are_suggested_because_they_are_what_people_type() {
        assert!(suggest("grey", 5).contains(&"grey"));
        assert!(suggest("clear", 5).contains(&"clear"));
    }

    #[test]
    fn a_word_nothing_resembles_suggests_nothing() {
        // Better to say "I don't know that colour" than to offer a wild guess.
        assert!(suggest("zzzzzzzzzz", 5).is_empty());
    }

    #[test]
    fn suggestions_are_stable_and_capped() {
        assert_eq!(suggest("blu", 3), suggest("blu", 3));
        assert!(suggest("dark", 3).len() <= 3);
        // An empty query is a listing, not a failure: it gives a picker
        // something to show before anything is typed.
        assert_eq!(suggest("", 5).len(), 5);
    }

    #[test]
    fn every_suggestion_resolves_to_a_real_colour() {
        for query in ["red", "dark", "yelow", "grey", "bluish", ""] {
            for name in suggest(query, 10) {
                assert!(
                    code_from_name(name).is_some(),
                    "{query:?} suggested {name:?}, which does not resolve"
                );
            }
        }
    }

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
    fn dark_nougat_is_recognised() {
        // The colour that started this: previously only Light Nougat and
        // Medium Nougat existed, so a real, common LEGO colour had no way in.
        assert_eq!(code_from_name("dark nougat"), Some(128));
        assert_eq!(name_of(128), Some("dark nougat"));
    }

    #[test]
    fn plain_nougat_and_violet_resolve_to_their_own_colour_not_the_light_one() {
        // "nougat" and "violet" name real LDraw colours of their own (92 and
        // 110) — a plain word must not quietly mean the *light* shade just
        // because that used to be the only one in the palette.
        assert_eq!(code_from_name("nougat"), Some(92));
        assert_eq!(code_from_name("light nougat"), Some(78));
        assert_eq!(code_from_name("violet"), Some(110));
        assert_eq!(code_from_name("light violet"), Some(20));
        // "Flesh" was LEGO's old name for Nougat, not Light Nougat.
        assert_eq!(code_from_name("flesh"), Some(92));
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
