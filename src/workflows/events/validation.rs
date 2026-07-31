use chrono::{TimeZone, Utc};
use chrono_tz::Europe::Helsinki;

pub fn format_zulip_topic(prefix: &str, start_date: &str, title: &str, uid: &str) -> String {
    let date_prefix = start_date.split_whitespace().next().unwrap_or(start_date);
    let topic = format!("{prefix} {date_prefix} {title} 🆔 {uid}");
    if topic.chars().count() > 60 {
        let static_len =
            prefix.chars().count() + date_prefix.chars().count() + uid.chars().count() + 5; // Spaces + 🆔
        let max_title_len = 60_usize.saturating_sub(static_len);
        let truncated_title = if max_title_len > 1 {
            format!(
                "{}…",
                title.chars().take(max_title_len - 1).collect::<String>()
            )
        } else {
            String::new()
        };
        format!("{prefix} {date_prefix} {truncated_title} 🆔 {uid}")
    } else {
        topic
    }
}

pub fn validate_event_dates(dates_str: &str, locale: &str) -> Result<(String, String), String> {
    let msg = |key: &str| rust_i18n::t!(key, locale = locale).to_string();

    if dates_str.trim().is_empty() {
        return Err(msg("validation.dates_blank"));
    }

    let start_str = if dates_str.len() >= 16 {
        &dates_str[0..16]
    } else {
        return Err(msg("validation.dates_start_format"));
    };

    let remainder = dates_str[16..].trim();
    let end_str = remainder.strip_prefix('-').unwrap_or(remainder).trim();

    let start_naive = chrono::NaiveDateTime::parse_from_str(start_str, "%Y-%m-%d %H:%M")
        .map_err(|_| msg("validation.dates_start_format"))?;

    let start_helsinki = Helsinki
        .from_local_datetime(&start_naive)
        .single()
        .ok_or_else(|| msg("validation.dates_start_invalid"))?;

    let end_naive = if end_str.is_empty() {
        start_naive + chrono::Duration::hours(1)
    } else if end_str.len() == 5 {
        let date_part = &start_str[0..10];
        let combined = format!("{date_part} {end_str}");
        chrono::NaiveDateTime::parse_from_str(&combined, "%Y-%m-%d %H:%M")
            .map_err(|_| msg("validation.dates_end_time_format"))?
    } else {
        chrono::NaiveDateTime::parse_from_str(end_str, "%Y-%m-%d %H:%M")
            .map_err(|_| msg("validation.dates_end_format"))?
    };

    let end_helsinki = Helsinki
        .from_local_datetime(&end_naive)
        .single()
        .ok_or_else(|| msg("validation.dates_end_invalid"))?;

    if end_helsinki <= start_helsinki {
        return Err(msg("validation.dates_end_before_start"));
    }

    Ok((
        start_helsinki.format("%Y-%m-%d %H:%M:00").to_string(),
        end_helsinki.format("%Y-%m-%d %H:%M:00").to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_event_dates() {
        // Valid dates
        assert!(validate_event_dates("2026-10-10 10:00 - 2026-10-10 12:00", "en-US").is_ok());

        // Blank input
        assert_eq!(
            validate_event_dates("", "en-US").unwrap_err(),
            "Dates cannot be blank."
        );

        // Blank end (defaults to 1 hour after start date)
        assert_eq!(
            validate_event_dates("2026-10-10 10:00", "en-US").unwrap(),
            (
                "2026-10-10 10:00:00".to_string(),
                "2026-10-10 11:00:00".to_string()
            )
        );

        // Short end time (defaults to same date)
        assert_eq!(
            validate_event_dates("2026-10-10 10:00 - 12:00", "en-US").unwrap(),
            (
                "2026-10-10 10:00:00".to_string(),
                "2026-10-10 12:00:00".to_string()
            )
        );

        // Invalid start format
        assert_eq!(
            validate_event_dates("2026-10-10", "en-US").unwrap_err(),
            "Start date must be in format YYYY-MM-DD HH:MM"
        );

        // Invalid end format
        assert_eq!(
            validate_event_dates("2026-10-10 10:00 - 2026/10/10 12:00", "en-US").unwrap_err(),
            "End date must be in format YYYY-MM-DD HH:MM"
        );

        // End date before start date
        assert_eq!(
            validate_event_dates("2026-10-10 14:00 - 2026-10-10 12:00", "en-US").unwrap_err(),
            "End date must be after the start date."
        );

        // End date equals start date
        assert_eq!(
            validate_event_dates("2026-10-10 12:00 - 2026-10-10 12:00", "en-US").unwrap_err(),
            "End date must be after the start date."
        );
    }

    #[test]
    fn test_validate_event_dates_is_localized() {
        let err = validate_event_dates("", "fi-FI").unwrap_err();
        assert_eq!(err, "Ajankohta ei voi olla tyhjä.");
    }

    #[test]
    fn test_format_zulip_topic() {
        let topic = format_zulip_topic("🆕", "2026-07-14 10:00", "Test Event", "12345");
        assert_eq!(topic, "🆕 2026-07-14 Test Event 🆔 12345");

        let topic2 = format_zulip_topic("📝", "2026-08-01", "Another Event", "67890");
        assert_eq!(topic2, "📝 2026-08-01 Another Event 🆔 67890");

        let long_title =
            "This is a very long title that will definitely exceed sixty characters in length";
        let topic3 = format_zulip_topic("🆕", "2026-07-14", long_title, "123456789012345");
        assert!(topic3.chars().count() <= 60);
        assert_eq!(
            topic3,
            "🆕 2026-07-14 This is a very long title th… 🆔 123456789012345"
        );
    }
}
