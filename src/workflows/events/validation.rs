use chrono::{TimeZone, Utc};
use chrono_tz::Europe::Helsinki;

pub fn validate_url(url: &str) -> Result<String, String> {
    let t = url.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    if !t.starts_with("http://") && !t.starts_with("https://") {
        return Err("URL must start with http:// or https://".to_string());
    }
    if t.len() > 200 {
        return Err("URL is too long (max 200 characters).".to_string());
    }
    Ok(t.to_string())
}

pub fn validate_text_field(
    val: &str,
    field_name: &str,
    max_len: usize,
    required: bool,
) -> Result<String, String> {
    let t = val.trim();
    if required && t.is_empty() {
        return Err(format!("{field_name} is required."));
    }
    if t.len() > max_len {
        return Err(format!(
            "{field_name} is too long (max {max_len} characters)."
        ));
    }
    Ok(t.to_string())
}

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

pub fn validate_event_dates(dates_str: &str) -> Result<(String, String), String> {
    if dates_str.trim().is_empty() {
        return Err("Dates cannot be blank.".to_string());
    }

    let start_str = if dates_str.len() >= 16 {
        &dates_str[0..16]
    } else {
        return Err("Start date must be in format YYYY-MM-DD HH:MM".to_string());
    };

    let remainder = dates_str[16..].trim();
    let end_str = remainder.strip_prefix('-').unwrap_or(remainder).trim();

    let start_naive = chrono::NaiveDateTime::parse_from_str(start_str, "%Y-%m-%d %H:%M")
        .map_err(|_| "Start date must be in format YYYY-MM-DD HH:MM".to_string())?;

    let start_helsinki = Helsinki
        .from_local_datetime(&start_naive)
        .single()
        .ok_or_else(|| "Invalid start time for Helsinki timezone.".to_string())?;

    let end_naive = if end_str.is_empty() {
        start_naive + chrono::Duration::hours(1)
    } else if end_str.len() == 5 {
        let date_part = &start_str[0..10];
        let combined = format!("{date_part} {end_str}");
        chrono::NaiveDateTime::parse_from_str(&combined, "%Y-%m-%d %H:%M")
            .map_err(|_| "End time must be in format HH:MM".to_string())?
    } else {
        chrono::NaiveDateTime::parse_from_str(end_str, "%Y-%m-%d %H:%M")
            .map_err(|_| "End date must be in format YYYY-MM-DD HH:MM".to_string())?
    };

    let end_helsinki = Helsinki
        .from_local_datetime(&end_naive)
        .single()
        .ok_or_else(|| "Invalid end time for Helsinki timezone.".to_string())?;

    if end_helsinki <= start_helsinki {
        return Err("End date must be after the start date.".to_string());
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
        assert!(validate_event_dates("2026-10-10 10:00 - 2026-10-10 12:00").is_ok());

        // Blank input
        assert_eq!(
            validate_event_dates("").unwrap_err(),
            "Dates cannot be blank."
        );

        // Blank end (defaults to 1 hour after start date)
        assert_eq!(
            validate_event_dates("2026-10-10 10:00").unwrap(),
            (
                "2026-10-10 10:00:00".to_string(),
                "2026-10-10 11:00:00".to_string()
            )
        );

        // Short end time (defaults to same date)
        assert_eq!(
            validate_event_dates("2026-10-10 10:00 - 12:00").unwrap(),
            (
                "2026-10-10 10:00:00".to_string(),
                "2026-10-10 12:00:00".to_string()
            )
        );

        // Invalid start format
        assert_eq!(
            validate_event_dates("2026-10-10").unwrap_err(),
            "Start date must be in format YYYY-MM-DD HH:MM"
        );

        // Invalid end format
        assert_eq!(
            validate_event_dates("2026-10-10 10:00 - 2026/10/10 12:00").unwrap_err(),
            "End date must be in format YYYY-MM-DD HH:MM"
        );

        // End date before start date
        assert_eq!(
            validate_event_dates("2026-10-10 14:00 - 2026-10-10 12:00").unwrap_err(),
            "End date must be after the start date."
        );

        // End date equals start date
        assert_eq!(
            validate_event_dates("2026-10-10 12:00 - 2026-10-10 12:00").unwrap_err(),
            "End date must be after the start date."
        );
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
