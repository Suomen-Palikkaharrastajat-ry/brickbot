#![allow(clippy::option_if_let_else)]
use aho_corasick::{AhoCorasick, MatchKind};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Topic {
    LegoSet,
    LegoPart,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

pub struct DetectionResult {
    pub topic: Topic,
    pub confidence: Confidence,
    pub extracted_id: Option<String>,
}

struct Keyword {
    term: &'static str,
    topic: Topic,
    weight: usize,
}

const KEYWORDS: &[Keyword] = &[
    // LegoSet
    Keyword {
        term: "lego set",
        topic: Topic::LegoSet,
        weight: 2,
    },
    Keyword {
        term: "lego setti",
        topic: Topic::LegoSet,
        weight: 2,
    },
    Keyword {
        term: "settejä",
        topic: Topic::LegoSet,
        weight: 2,
    },
    Keyword {
        term: "setin",
        topic: Topic::LegoSet,
        weight: 1,
    },
    Keyword {
        term: "rakennussarj",
        topic: Topic::LegoSet,
        weight: 2,
    },
    Keyword {
        term: "build",
        topic: Topic::LegoSet,
        weight: 1,
    },
    Keyword {
        term: "rakentaa",
        topic: Topic::LegoSet,
        weight: 1,
    },
    // LegoPart
    Keyword {
        term: "part",
        topic: Topic::LegoPart,
        weight: 2,
    },
    Keyword {
        term: "osa",
        topic: Topic::LegoPart,
        weight: 2,
    },
    Keyword {
        term: "osia",
        topic: Topic::LegoPart,
        weight: 2,
    },
    Keyword {
        term: "piece",
        topic: Topic::LegoPart,
        weight: 1,
    },
    Keyword {
        term: "brick",
        topic: Topic::LegoPart,
        weight: 1,
    },
    Keyword {
        term: "palik",
        topic: Topic::LegoPart,
        weight: 1,
    },
    Keyword {
        term: "element",
        topic: Topic::LegoPart,
        weight: 1,
    },
];

static AC_ENGINE: OnceLock<AhoCorasick> = OnceLock::new();
static SET_NUM_REGEX: OnceLock<Regex> = OnceLock::new();

pub fn detect_topic(content: &str, log_ambient: bool) -> Option<DetectionResult> {
    let content = content.to_lowercase();

    let engine = AC_ENGINE.get_or_init(|| {
        let patterns: Vec<&str> = KEYWORDS.iter().map(|k| k.term).collect();
        AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns)
            .unwrap()
    });

    let set_re = SET_NUM_REGEX
        .get_or_init(|| Regex::new(r"\b(?:2[1-9]\d{2}|[3-9]\d{3}|\d{5,7})\b").unwrap());

    let mut set_score = 0;
    let mut part_score = 0;

    if log_ambient {
        tracing::info!("--- Ambient Topic Detection Started ---");
        tracing::info!("Content: '{}'", content);
    }

    for mat in engine.find_iter(&content) {
        let kw = &KEYWORDS[mat.pattern().as_usize()];
        if log_ambient {
            tracing::info!(
                "Found keyword '{}' (topic: {:?}, weight: {})",
                kw.term,
                kw.topic,
                kw.weight
            );
        }
        match kw.topic {
            Topic::LegoSet => set_score += kw.weight,
            Topic::LegoPart => part_score += kw.weight,
        }
    }

    let mut extracted_id = None;

    if let Some(m) = set_re.find(&content) {
        set_score += 2;
        extracted_id = Some(m.as_str().to_string());
        if log_ambient {
            tracing::info!("Found Set ID match '{}' (+2 to LegoSet)", m.as_str());
        }
    }

    let scores = [(Topic::LegoPart, part_score), (Topic::LegoSet, set_score)];

    let mut best_topic = None;
    let mut highest_score = 0;

    for &(topic, score) in &scores {
        if score > highest_score {
            highest_score = score;
            best_topic = Some(topic);
        }
    }

    if log_ambient {
        tracing::info!("Final scores: {:?}", scores);
        tracing::info!("Best topic: {:?} (Score: {})", best_topic, highest_score);
    }

    if let Some(topic) = best_topic {
        let confidence = if highest_score >= 4 {
            Confidence::High
        } else if highest_score >= 2 {
            Confidence::Medium
        } else {
            Confidence::Low
        };

        if log_ambient {
            tracing::info!("Confidence level: {:?}", confidence);
            tracing::info!("--- Ambient Topic Detection Ended ---");
        }

        Some(DetectionResult {
            topic,
            confidence,
            extracted_id,
        })
    } else {
        if log_ambient {
            tracing::info!("No topic detected.");
            tracing::info!("--- Ambient Topic Detection Ended ---");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_topic_low_confidence() {
        let res = detect_topic("I bought a new lego set yesterday.", false);
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.topic, Topic::LegoSet);
        assert_eq!(res.confidence, Confidence::Medium); // "lego set" is weight 2 -> Medium
    }

    #[test]
    fn test_detect_topic_high_confidence() {
        let res = detect_topic(
            "I need a part, where can I find this part? It's a rare piece.",
            false,
        );
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.topic, Topic::LegoPart);
        // part (2) + part (2) + piece (1) = 5 -> High
        assert_eq!(res.confidence, Confidence::High);
    }

    #[test]
    fn test_detect_topic_finnish() {
        let res = detect_topic("Mistä löydän osan 3001?", false);
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.topic, Topic::LegoPart);
    }

    #[test]
    fn test_detect_set_number() {
        let res = detect_topic("I got 42083 for my birthday.", false);
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.topic, Topic::LegoSet);
        assert_eq!(res.confidence, Confidence::Medium); // 2 score = Medium

        // Should not detect years <= 2099
        let res2 = detect_topic("The year is 2099.", false);
        assert!(res2.is_none());

        // Should detect 2100 and above
        let res3 = detect_topic("I found set 2100 yesterday.", false);
        assert!(res3.is_some());
        assert_eq!(res3.unwrap().topic, Topic::LegoSet);
    }
}
