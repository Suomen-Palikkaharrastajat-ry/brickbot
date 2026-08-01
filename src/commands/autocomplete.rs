//! Typed-ahead suggestions for colour and part options.
//!
//! Autocomplete is the only live typed-ahead Discord offers, and it fires on
//! **every keystroke** with a three-second deadline and no defer. So nothing
//! here ever waits for the network: colours come from the local palette, and
//! parts come from the built-in catalogue plus whatever the sticky cache has
//! already learnt. A query the cache does not know starts a background fetch
//! and answers with what is on hand — the keystroke that misses pays nothing,
//! and the next one is fast.
//!
//! The choice building is pure and tested here; the routing lives in
//! `src/main.rs`.

use crate::build::parts;
use crate::discord_limits::truncate;
use crate::ldraw::colors;
use crate::parts_cache::CachedPart;

/// Discord accepts at most this many choices, and caps each name here.
const MAX_CHOICES: usize = 25;
const MAX_CHOICE_NAME: usize = 100;

/// One suggestion: what the builder sees, and what gets inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    pub value: String,
}

/// Colour suggestions, entirely from the local palette.
///
/// Needs no cache and no network — the 74 `LDraw` colours are compiled in — so
/// this half works on a cold start and with no API key at all.
#[must_use]
pub fn colour_choices(query: &str) -> Vec<Choice> {
    colors::suggest(query, MAX_CHOICES)
        .into_iter()
        .map(|name| Choice {
            name: truncate(name, MAX_CHOICE_NAME),
            // The value is the colour word itself, so it drops straight into a
            // legend line and resolves through the ordinary grammar.
            value: name.to_string(),
        })
        .collect()
}

/// Part suggestions: the built-in catalogue first, then the sticky cache.
///
/// The catalogue answers the everyday sizes instantly and offline; the cache
/// answers everything the guild has looked up before. Duplicates are dropped
/// by part number, catalogue first, because a size we can place without a
/// lookup is a better suggestion than one we cannot.
#[must_use]
pub fn part_choices(query: &str, cached: &[CachedPart]) -> Vec<Choice> {
    let mut choices: Vec<Choice> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for part in catalogue_matches(query) {
        if !seen.contains(&part.part_num) {
            seen.push(part.part_num.clone());
            choices.push(part.into());
        }
    }
    for part in cached {
        if choices.len() >= MAX_CHOICES {
            break;
        }
        if !seen.contains(&part.part_num) {
            seen.push(part.part_num.clone());
            choices.push(part.clone().into());
        }
    }

    choices.truncate(MAX_CHOICES);
    choices
}

impl From<CachedPart> for Choice {
    fn from(part: CachedPart) -> Self {
        // Named the way `/part` names them, and truncated char-safely: a part
        // name with a multi-byte character must not panic the handler.
        Self {
            name: truncate(
                &format!("{} - {}", part.part_num, part.name),
                MAX_CHOICE_NAME,
            ),
            value: part.part_num,
        }
    }
}

/// Everyday parts whose name or number matches, from the compiled-in
/// catalogue.
fn catalogue_matches(query: &str) -> Vec<CachedPart> {
    let needle = crate::parts_cache::normalize(query);
    if needle.is_empty() {
        return Vec::new();
    }
    parts::catalogue(None)
        .filter(|part| {
            part.name.to_lowercase().contains(&needle)
                || part.ldraw_id.to_lowercase().contains(&needle)
                // `2x4` is how people type it; the catalogue spells it `2 x 4`.
                || part.name.to_lowercase().replace(" x ", "x").contains(&needle)
        })
        .map(|part| CachedPart {
            part_num: part.ldraw_id,
            name: part.name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(part_num: &str, name: &str) -> CachedPart {
        CachedPart {
            part_num: part_num.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn a_colour_suggests_itself_and_its_neighbours() {
        let choices = colour_choices("dark");
        let names: Vec<&str> = choices.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"dark tan"), "got: {names:?}");
        // The value is the word itself, so it can be pasted into a legend.
        assert!(choices.iter().all(|c| c.name == c.value));
    }

    #[test]
    fn a_misspelled_colour_still_suggests_something() {
        let choices = colour_choices("yelow");
        assert!(
            choices.iter().any(|c| c.value == "yellow"),
            "got: {choices:?}"
        );
    }

    #[test]
    fn colour_suggestions_stay_within_discords_limits() {
        for query in ["", "d", "dark", "grey", "zzzz"] {
            let choices = colour_choices(query);
            assert!(choices.len() <= MAX_CHOICES, "{query:?}: {}", choices.len());
            for choice in choices {
                assert!(choice.name.chars().count() <= MAX_CHOICE_NAME);
                assert!(!choice.name.is_empty());
            }
        }
    }

    #[test]
    fn the_catalogue_answers_an_everyday_size_with_no_cache_at_all() {
        // The cold-start case: nothing has ever been looked up.
        let choices = part_choices("2x4", &[]);
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"3001"), "brick 2x4 missing: {values:?}");
        assert!(values.contains(&"3020"), "plate 2x4 missing: {values:?}");
    }

    #[test]
    fn a_part_number_finds_its_own_part() {
        let choices = part_choices("3001", &[]);
        assert_eq!(choices.first().map(|c| c.value.as_str()), Some("3001"));
    }

    #[test]
    fn cached_parts_extend_the_catalogue_rather_than_replacing_it() {
        let choices = part_choices("brick", &[cached("2437", "Windscreen Brick Thing")]);
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"2437"), "cache missing: {values:?}");
        assert!(values.contains(&"3001"), "catalogue missing: {values:?}");
    }

    #[test]
    fn a_part_in_both_the_catalogue_and_the_cache_is_offered_once() {
        let choices = part_choices("2x4", &[cached("3001", "Brick 2 x 4")]);
        let hits = choices.iter().filter(|c| c.value == "3001").count();
        assert_eq!(hits, 1, "got: {choices:?}");
    }

    #[test]
    fn part_suggestions_stay_within_discords_limits() {
        let many: Vec<CachedPart> = (0..60)
            .map(|i| cached(&format!("p{i}"), &format!("Brick Variant {i}")))
            .collect();
        let choices = part_choices("brick", &many);
        assert!(choices.len() <= MAX_CHOICES, "got {}", choices.len());
    }

    #[test]
    fn a_multibyte_part_name_is_truncated_without_panicking() {
        // `/part` slices bytes and would panic here.
        let long = "Ä".repeat(200);
        let choices = part_choices("brick", &[cached("9999", &long)]);
        for choice in choices {
            assert!(choice.name.chars().count() <= MAX_CHOICE_NAME);
        }
    }

    #[test]
    fn an_empty_query_suggests_no_parts() {
        // Nothing typed yet: the catalogue would otherwise dump all 38.
        assert!(part_choices("", &[]).is_empty());
        assert!(part_choices("   ", &[]).is_empty());
    }
}
