use crate::workflows::locations::models::{GeoPoint, LOCATION_TAGS, MAX_LOCATION_TAGS};

/// Maximum length of a Zulip topic we are willing to generate.
const MAX_TOPIC_LEN: usize = 60;

/// Builds the Zulip moderation topic for a location.
///
/// Locations carry no start date, so unlike [`crate::workflows::events::format_zulip_topic`]
/// the topic is `"{prefix} {title} 🆔 {uid}"`, still capped at 60 characters.
#[must_use]
pub fn format_location_zulip_topic(prefix: &str, title: &str, uid: &str) -> String {
    let topic = format!("{prefix} {title} 🆔 {uid}");
    if topic.chars().count() <= MAX_TOPIC_LEN {
        return topic;
    }

    // Spaces around the title and the 🆔 marker.
    let static_len = prefix.chars().count() + uid.chars().count() + 4;
    let max_title_len = MAX_TOPIC_LEN.saturating_sub(static_len);
    let truncated_title = if max_title_len > 1 {
        format!(
            "{}…",
            title.chars().take(max_title_len - 1).collect::<String>()
        )
    } else {
        String::new()
    };
    format!("{prefix} {truncated_title} 🆔 {uid}")
}

/// Reads a usable `point` out of a moderation payload.
///
/// Accepts both JSON numbers and numeric strings, since moderators type these by hand into a
/// YAML block. Returns `None` for a missing, null, zero or out-of-range point.
#[must_use]
pub fn extract_point(payload: &serde_json::Value) -> Option<GeoPoint> {
    let point = payload.get("point")?;
    let read = |key: &str| -> Option<f64> {
        let v = point.get(key)?;
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
    };

    let candidate = GeoPoint {
        lat: read("lat")?,
        lon: read("lon")?,
    };
    candidate.is_set().then_some(candidate)
}

/// Keeps only known tag values, deduplicated, capped at the collection's `maxSelect`.
#[must_use]
pub fn sanitize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let t = tag.trim();
        if LOCATION_TAGS.contains(&t) && !out.iter().any(|existing| existing == t) {
            out.push(t.to_string());
        }
        if out.len() >= MAX_LOCATION_TAGS as usize {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_location_zulip_topic() {
        assert_eq!(
            format_location_zulip_topic("📍", "Legokauppa", "12345"),
            "📍 Legokauppa 🆔 12345"
        );
        assert_eq!(
            format_location_zulip_topic("📌", "Museo", "67890"),
            "📌 Museo 🆔 67890"
        );

        let long_title =
            "This is a very long location title that will definitely exceed sixty characters";
        let topic = format_location_zulip_topic("📍", long_title, "123456789012345");
        assert!(topic.chars().count() <= 60);
        assert!(topic.starts_with("📍 This is a very long location"));
        assert!(topic.ends_with("🆔 123456789012345"));
    }

    #[test]
    fn test_extract_point() {
        let payload = serde_json::json!({"point": {"lat": 60.1699, "lon": 24.9384}});
        assert_eq!(
            extract_point(&payload),
            Some(GeoPoint {
                lat: 60.1699,
                lon: 24.9384
            })
        );

        // Moderators may type the numbers as quoted strings in YAML.
        let as_strings = serde_json::json!({"point": {"lat": "60.1699", "lon": "24.9384"}});
        assert!(extract_point(&as_strings).is_some());

        assert_eq!(extract_point(&serde_json::json!({})), None);
        assert_eq!(extract_point(&serde_json::json!({"point": null})), None);
        assert_eq!(
            extract_point(&serde_json::json!({"point": {"lat": null, "lon": null}})),
            None
        );
        assert_eq!(
            extract_point(&serde_json::json!({"point": {"lat": 0, "lon": 0}})),
            None
        );
        assert_eq!(
            extract_point(&serde_json::json!({"point": {"lat": 60.1}})),
            None
        );
        assert_eq!(
            extract_point(&serde_json::json!({"point": {"lat": 100.0, "lon": 24.0}})),
            None
        );
    }

    #[test]
    fn test_sanitize_tags() {
        assert_eq!(
            sanitize_tags(&["store".to_string(), "museum".to_string()]),
            vec!["store".to_string(), "museum".to_string()]
        );
        // Unknown values are dropped.
        assert_eq!(
            sanitize_tags(&["competition".to_string()]),
            Vec::<String>::new()
        );
        // Duplicates collapse.
        assert_eq!(
            sanitize_tags(&["store".to_string(), "store".to_string()]),
            vec!["store".to_string()]
        );
        // Never more than the collection's maxSelect.
        assert_eq!(
            sanitize_tags(&[
                "store".to_string(),
                "museum".to_string(),
                "other".to_string()
            ])
            .len(),
            MAX_LOCATION_TAGS as usize
        );
    }
}
