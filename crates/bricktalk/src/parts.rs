//! Resolving a described part to a real element.
//!
//! Most sentences name an everyday brick — `a red 2x4 brick` — so a small
//! built-in catalogue answers them instantly, with no network round trip and
//! no ambiguity to put to the builder. Anything the catalogue does not cover
//! falls through to the Rebrickable lookup that already backs `/part`.

#![allow(clippy::doc_markdown)]

use super::dsl::ast::PartSpec;
use super::model::{PartKind, PartRef};

/// One catalogue entry: a shape and size, and the LDraw part that is it.
struct Entry {
    kind: PartKind,
    width: u8,
    depth: u8,
    ldraw_id: &'static str,
    name: &'static str,
}

const fn e(
    kind: PartKind,
    width: u8,
    depth: u8,
    ldraw_id: &'static str,
    name: &'static str,
) -> Entry {
    Entry {
        kind,
        width,
        depth,
        ldraw_id,
        name,
    }
}

/// The everyday System elements, by LDraw design id.
///
/// Sizes are stored with the smaller dimension first; [`lookup`] normalises
/// the query the same way, so `4x2` finds the `2x4` brick.
#[rustfmt::skip]
static CATALOG: &[Entry] = &[
    // Bricks
    e(PartKind::Brick, 1, 1,  "3005",  "Brick 1 x 1"),
    e(PartKind::Brick, 1, 2,  "3004",  "Brick 1 x 2"),
    e(PartKind::Brick, 1, 3,  "3622",  "Brick 1 x 3"),
    e(PartKind::Brick, 1, 4,  "3010",  "Brick 1 x 4"),
    e(PartKind::Brick, 1, 6,  "3009",  "Brick 1 x 6"),
    e(PartKind::Brick, 1, 8,  "3008",  "Brick 1 x 8"),
    e(PartKind::Brick, 2, 2,  "3003",  "Brick 2 x 2"),
    e(PartKind::Brick, 2, 3,  "3002",  "Brick 2 x 3"),
    e(PartKind::Brick, 2, 4,  "3001",  "Brick 2 x 4"),
    e(PartKind::Brick, 2, 6,  "2456",  "Brick 2 x 6"),
    e(PartKind::Brick, 2, 8,  "3007",  "Brick 2 x 8"),
    e(PartKind::Brick, 2, 10, "3006",  "Brick 2 x 10"),
    // Plates
    e(PartKind::Plate, 1, 1,  "3024",  "Plate 1 x 1"),
    e(PartKind::Plate, 1, 2,  "3023",  "Plate 1 x 2"),
    e(PartKind::Plate, 1, 3,  "3623",  "Plate 1 x 3"),
    e(PartKind::Plate, 1, 4,  "3710",  "Plate 1 x 4"),
    e(PartKind::Plate, 1, 6,  "3666",  "Plate 1 x 6"),
    e(PartKind::Plate, 1, 8,  "3460",  "Plate 1 x 8"),
    e(PartKind::Plate, 2, 2,  "3022",  "Plate 2 x 2"),
    e(PartKind::Plate, 2, 3,  "3021",  "Plate 2 x 3"),
    e(PartKind::Plate, 2, 4,  "3020",  "Plate 2 x 4"),
    e(PartKind::Plate, 2, 6,  "3795",  "Plate 2 x 6"),
    e(PartKind::Plate, 2, 8,  "3034",  "Plate 2 x 8"),
    e(PartKind::Plate, 4, 4,  "3031",  "Plate 4 x 4"),
    e(PartKind::Plate, 4, 6,  "3032",  "Plate 4 x 6"),
    e(PartKind::Plate, 4, 8,  "3035",  "Plate 4 x 8"),
    e(PartKind::Plate, 6, 6,  "3958",  "Plate 6 x 6"),
    e(PartKind::Plate, 6, 8,  "3036",  "Plate 6 x 8"),
    e(PartKind::Plate, 8, 8,  "41539", "Plate 8 x 8"),
    // Tiles
    e(PartKind::Tile,  1, 1,  "3070b", "Tile 1 x 1"),
    e(PartKind::Tile,  1, 2,  "3069b", "Tile 1 x 2"),
    e(PartKind::Tile,  1, 4,  "2431",  "Tile 1 x 4"),
    e(PartKind::Tile,  2, 2,  "3068b", "Tile 2 x 2"),
    e(PartKind::Tile,  2, 4,  "87079", "Tile 2 x 4"),
    // Slopes (45 degrees)
    e(PartKind::Slope, 1, 2,  "3040",  "Slope 45 1 x 2"),
    e(PartKind::Slope, 2, 2,  "3039",  "Slope 45 2 x 2"),
    e(PartKind::Slope, 2, 3,  "3038",  "Slope 45 2 x 3"),
    e(PartKind::Slope, 2, 4,  "3037",  "Slope 45 2 x 4"),
];

/// What a described part resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Found in the built-in catalogue; no lookup needed.
    Known(PartRef),
    /// Not in the catalogue. The caller should search for `query`, which is
    /// what the existing `/part` workflow does.
    NeedsLookup { query: String, kind: PartKind },
}

/// The part used when a sentence names no size or shape at all.
#[must_use]
pub fn default_part() -> PartRef {
    PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick)
}

/// Looks up an exact shape and size in the catalogue.
#[must_use]
pub fn lookup(kind: PartKind, width: u8, depth: u8) -> Option<PartRef> {
    // Normalise so 4x2 and 2x4 are the same query.
    let (small, large) = if width <= depth {
        (width, depth)
    } else {
        (depth, width)
    };
    CATALOG
        .iter()
        .find(|entry| entry.kind == kind && entry.width == small && entry.depth == large)
        .map(|entry| {
            // Report the footprint as the builder asked for it, so a part
            // rotated in the sentence keeps its intended orientation.
            PartRef::new(entry.ldraw_id, entry.name, (width, depth), entry.kind)
        })
}

/// The catalogue, optionally narrowed to one shape.
///
/// For pickers that show what is available rather than answering a query.
pub fn catalogue(kind: Option<PartKind>) -> impl Iterator<Item = PartRef> {
    CATALOG
        .iter()
        .filter(move |entry| kind.is_none_or(|wanted| entry.kind == wanted))
        .map(|entry| {
            PartRef::new(
                entry.ldraw_id,
                entry.name,
                (entry.width, entry.depth),
                entry.kind,
            )
        })
}

/// The catalogue parts of a shape closest to a size that is not in it.
///
/// `1x7 brick` is not a real element, and answering "I have 1x6 and 1x8" is
/// far more use than a network round trip for something that cannot be found.
/// Sizes are compared normalised, so `7x1` and `1x7` suggest the same things.
#[must_use]
pub fn suggest(kind: PartKind, width: u8, depth: u8, limit: usize) -> Vec<PartRef> {
    let (small, large) = if width <= depth {
        (width, depth)
    } else {
        (depth, width)
    };

    let mut scored: Vec<(u16, &Entry)> = CATALOG
        .iter()
        .filter(|entry| entry.kind == kind)
        .map(|entry| {
            let distance =
                u16::from(entry.width.abs_diff(small)) + u16::from(entry.depth.abs_diff(large));
            (distance, entry)
        })
        .collect();

    // Nearest first; ties break on the smaller part, then on part number, so
    // the same miss always suggests the same alternatives.
    scored.sort_by_key(|(distance, entry)| (*distance, entry.width, entry.depth, entry.ldraw_id));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, entry)| {
            PartRef::new(
                entry.ldraw_id,
                entry.name,
                (entry.width, entry.depth),
                entry.kind,
            )
        })
        .collect()
}

/// Resolves a described part.
///
/// An explicit part number is taken at face value: the builder named a
/// specific element, so we do not second-guess it. Otherwise the catalogue is
/// consulted, and only a genuinely unusual part falls through to a lookup.
#[must_use]
pub fn resolve(spec: &PartSpec) -> Resolution {
    let kind = spec.kind.unwrap_or(PartKind::Brick);

    if let Some(part_num) = &spec.part_num {
        // A bare part number has no footprint we can infer, which excludes it
        // from collision detection until a lookup fills it in.
        return Resolution::NeedsLookup {
            query: part_num.clone(),
            kind,
        };
    }

    if let Some((w, d)) = spec.size {
        lookup(kind, w, d).map_or_else(
            || Resolution::NeedsLookup {
                query: spec.search_query(),
                kind,
            },
            Resolution::Known,
        )
    } else {
        // No size given: fall back to the most iconic element of that shape.
        {
            let default_size = match kind {
                PartKind::Tile => (2u8, 2u8),
                PartKind::Brick | PartKind::Plate | PartKind::Slope | PartKind::Other => (2, 4),
            };
            lookup(kind, default_size.0, default_size.1)
                .map_or_else(|| Resolution::Known(default_part()), Resolution::Known)
        }
    }
}

/// Builds a `PartRef` from a Rebrickable lookup result.
///
/// The LDraw id comes from Rebrickable's `external_ids`, which already
/// deserialises in `crate::brick::RebrickablePart` (in the binary); when it is absent we
/// fall back to the Rebrickable number, which matches the LDraw design id for
/// most System parts.
#[must_use]
pub fn from_rebrickable<S: std::hash::BuildHasher>(
    part_num: &str,
    name: &str,
    external_ids: &std::collections::HashMap<String, Vec<String>, S>,
    kind: PartKind,
) -> PartRef {
    let ldraw_id = external_ids
        .get("LDraw")
        .and_then(|ids| ids.first())
        .cloned()
        .unwrap_or_else(|| part_num.to_string());

    let mut part = PartRef::new(
        ldraw_id,
        name,
        footprint_from_name(name).unwrap_or((0, 0)),
        kind_from_name(name).unwrap_or(kind),
    );
    part.rebrickable_id = Some(part_num.to_string());
    part
}

/// Reads a footprint out of a catalogue name such as `Brick 2 x 4`.
#[must_use]
pub fn footprint_from_name(name: &str) -> Option<(u8, u8)> {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    for pair in words.windows(3) {
        if pair[1] == "x" {
            if let (Ok(w), Ok(d)) = (pair[0].parse::<u8>(), pair[2].parse::<u8>()) {
                return Some((w, d));
            }
        }
    }
    // Also accept the compact "2x4" spelling.
    for word in words {
        if let Some((w, d)) = word.split_once('x') {
            if let (Ok(w), Ok(d)) = (w.parse::<u8>(), d.parse::<u8>()) {
                return Some((w, d));
            }
        }
    }
    None
}

/// Reads a shape out of a catalogue name.
#[must_use]
pub fn kind_from_name(name: &str) -> Option<PartKind> {
    let lower = name.to_lowercase();
    // Check the more specific shapes first: "Slope Brick 45 2 x 4" is a slope,
    // not a brick.
    for (needle, kind) in [
        ("slope", PartKind::Slope),
        ("tile", PartKind::Tile),
        ("plate", PartKind::Plate),
        ("brick", PartKind::Brick),
    ] {
        if lower.contains(needle) {
            return Some(kind);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_that_does_not_exist_suggests_the_ones_that_do() {
        // The case that sends people to a fruitless Rebrickable search: 1x7
        // is not a brick, but 1x6 and 1x8 are.
        let suggested = suggest(PartKind::Brick, 1, 7, 3);
        let sizes: Vec<(u8, u8)> = suggested.iter().map(|p| p.footprint).collect();
        assert!(sizes.contains(&(1, 6)), "got: {sizes:?}");
        assert!(sizes.contains(&(1, 8)), "got: {sizes:?}");
    }

    #[test]
    fn suggestions_keep_to_the_shape_that_was_asked_for() {
        for part in suggest(PartKind::Plate, 3, 9, 5) {
            assert_eq!(part.kind, PartKind::Plate, "got a {:?}", part.kind);
        }
        for part in suggest(PartKind::Tile, 5, 5, 5) {
            assert_eq!(part.kind, PartKind::Tile);
        }
    }

    #[test]
    fn a_size_is_suggested_the_same_way_round_as_its_mirror() {
        // 7x1 and 1x7 are the same request.
        assert_eq!(
            suggest(PartKind::Brick, 7, 1, 3),
            suggest(PartKind::Brick, 1, 7, 3)
        );
    }

    #[test]
    fn the_nearest_size_comes_first_and_the_list_is_capped() {
        // 2x5 sits between 2x4 and 2x6, and both are one step away; the
        // smaller wins the tie so the answer never wobbles.
        let suggested = suggest(PartKind::Brick, 2, 5, 2);
        assert_eq!(suggested.len(), 2);
        assert_eq!(suggested[0].footprint, (2, 4));
        assert_eq!(suggested[1].footprint, (2, 6));
        assert_eq!(suggest(PartKind::Brick, 2, 5, 2), suggested);
    }

    #[test]
    fn the_catalogue_can_be_listed_whole_or_by_shape() {
        let everything: Vec<_> = catalogue(None).collect();
        assert_eq!(everything.len(), CATALOG.len());

        let tiles: Vec<_> = catalogue(Some(PartKind::Tile)).collect();
        assert!(!tiles.is_empty());
        assert!(tiles.iter().all(|p| p.kind == PartKind::Tile));
        assert!(tiles.len() < everything.len());

        // Everything listed can actually be placed.
        for part in everything {
            assert!(
                lookup(part.kind, part.footprint.0, part.footprint.1).is_some(),
                "{} is listed but does not look up",
                part.ldraw_id
            );
        }
    }

    #[test]
    fn finds_everyday_bricks_in_the_catalogue() {
        let brick = lookup(PartKind::Brick, 2, 4).unwrap();
        assert_eq!(brick.ldraw_id, "3001");
        assert_eq!(brick.name, "Brick 2 x 4");
        assert_eq!(brick.footprint, (2, 4));

        assert_eq!(lookup(PartKind::Plate, 8, 8).unwrap().ldraw_id, "41539");
        assert_eq!(lookup(PartKind::Tile, 1, 2).unwrap().ldraw_id, "3069b");
        assert_eq!(lookup(PartKind::Slope, 2, 4).unwrap().ldraw_id, "3037");
    }

    #[test]
    fn size_order_does_not_matter_but_footprint_is_preserved() {
        let a = lookup(PartKind::Brick, 2, 4).unwrap();
        let b = lookup(PartKind::Brick, 4, 2).unwrap();
        assert_eq!(a.ldraw_id, b.ldraw_id);
        // The footprint reflects how the builder said it, so a 4x2 stays wide.
        assert_eq!(a.footprint, (2, 4));
        assert_eq!(b.footprint, (4, 2));
    }

    #[test]
    fn an_unstocked_size_is_not_in_the_catalogue() {
        assert!(lookup(PartKind::Brick, 7, 13).is_none());
    }

    #[test]
    fn resolves_a_described_part_without_a_lookup() {
        let spec = PartSpec {
            size: Some((2, 4)),
            kind: Some(PartKind::Brick),
            quantity: 1,
            ..PartSpec::default()
        };
        match resolve(&spec) {
            Resolution::Known(part) => assert_eq!(part.ldraw_id, "3001"),
            Resolution::NeedsLookup { query, .. } => {
                panic!("expected a known part, got a lookup for {query:?}")
            }
        }
    }

    #[test]
    fn a_part_with_no_size_falls_back_to_an_iconic_element() {
        let brick = PartSpec {
            kind: Some(PartKind::Brick),
            quantity: 1,
            ..PartSpec::default()
        };
        match resolve(&brick) {
            Resolution::Known(part) => assert_eq!(part.ldraw_id, "3001"),
            Resolution::NeedsLookup { query, .. } => {
                panic!("expected a known part, got a lookup for {query:?}")
            }
        }

        let tile = PartSpec {
            kind: Some(PartKind::Tile),
            quantity: 1,
            ..PartSpec::default()
        };
        match resolve(&tile) {
            Resolution::Known(part) => assert_eq!(part.ldraw_id, "3068b"),
            Resolution::NeedsLookup { query, .. } => {
                panic!("expected a known part, got a lookup for {query:?}")
            }
        }
    }

    #[test]
    fn an_unusual_size_asks_for_a_lookup() {
        let spec = PartSpec {
            size: Some((3, 12)),
            kind: Some(PartKind::Brick),
            quantity: 1,
            ..PartSpec::default()
        };
        assert_eq!(
            resolve(&spec),
            Resolution::NeedsLookup {
                query: "brick 3 x 12".to_string(),
                kind: PartKind::Brick,
            }
        );
    }

    #[test]
    fn an_explicit_part_number_always_goes_to_lookup() {
        let spec = PartSpec {
            part_num: Some("4073".to_string()),
            quantity: 1,
            ..PartSpec::default()
        };
        assert_eq!(
            resolve(&spec),
            Resolution::NeedsLookup {
                query: "4073".to_string(),
                kind: PartKind::Brick,
            }
        );
    }

    #[test]
    fn reads_footprints_out_of_catalogue_names() {
        assert_eq!(footprint_from_name("Brick 2 x 4"), Some((2, 4)));
        assert_eq!(footprint_from_name("Plate 1 x 10"), Some((1, 10)));
        assert_eq!(footprint_from_name("Brick 2x4"), Some((2, 4)));
        assert_eq!(footprint_from_name("Wheel 30mm"), None);
    }

    #[test]
    fn reads_shapes_out_of_catalogue_names() {
        assert_eq!(kind_from_name("Brick 2 x 4"), Some(PartKind::Brick));
        assert_eq!(kind_from_name("Plate 2 x 4"), Some(PartKind::Plate));
        // "Slope Brick 45 2 x 4" contains both words; the slope must win.
        assert_eq!(
            kind_from_name("Slope Brick 45 2 x 4"),
            Some(PartKind::Slope)
        );
        assert_eq!(kind_from_name("Minifig Head"), None);
    }

    #[test]
    fn builds_a_part_from_a_rebrickable_result() {
        let mut ids = std::collections::HashMap::new();
        ids.insert("LDraw".to_string(), vec!["3001".to_string()]);
        ids.insert("BrickLink".to_string(), vec!["3001".to_string()]);

        let part = from_rebrickable("3001", "Brick 2 x 4", &ids, PartKind::Brick);
        assert_eq!(part.ldraw_id, "3001");
        assert_eq!(part.rebrickable_id.as_deref(), Some("3001"));
        assert_eq!(part.footprint, (2, 4));
        assert_eq!(part.kind, PartKind::Brick);
    }

    #[test]
    fn falls_back_to_the_rebrickable_number_when_no_ldraw_id_is_given() {
        let ids = std::collections::HashMap::new();
        let part = from_rebrickable("98765", "Wheel 30mm", &ids, PartKind::Other);
        assert_eq!(part.ldraw_id, "98765");
        // No footprint could be read, so it is excluded from collisions.
        assert!(!part.has_footprint());
    }

    #[test]
    fn every_catalogue_entry_is_self_consistent() {
        for entry in CATALOG {
            assert!(
                entry.width <= entry.depth,
                "{} is stored with its larger dimension first",
                entry.ldraw_id
            );
            assert_eq!(
                kind_from_name(entry.name),
                Some(entry.kind),
                "{} has a name that does not match its kind",
                entry.ldraw_id
            );
            assert_eq!(
                footprint_from_name(entry.name),
                Some((entry.width, entry.depth)),
                "{} has a name that does not match its size",
                entry.ldraw_id
            );
        }
    }

    #[test]
    fn catalogue_ids_are_unique() {
        for (i, entry) in CATALOG.iter().enumerate() {
            for other in CATALOG.iter().skip(i + 1) {
                assert_ne!(
                    entry.ldraw_id, other.ldraw_id,
                    "duplicate id {}",
                    entry.ldraw_id
                );
            }
        }
    }
}
