//! Validators shared by the events and locations wizards.
//!
//! The messages are localized: a user who filled in a Finnish modal gets Finnish errors, with
//! the same field label the modal showed them.

/// Validates a free-text modal field and returns the trimmed value.
///
/// `field_label` is the already-localized label of the field, so it reads the same in the error
/// as it did in the modal.
pub fn validate_text_field(
    val: &str,
    field_label: &str,
    max_len: usize,
    required: bool,
    locale: &str,
) -> Result<String, String> {
    let t = val.trim();
    if required && t.is_empty() {
        return Err(
            rust_i18n::t!("validation.required", locale = locale, field = field_label).to_string(),
        );
    }
    if t.len() > max_len {
        return Err(rust_i18n::t!(
            "validation.too_long",
            locale = locale,
            field = field_label,
            max = max_len.to_string()
        )
        .to_string());
    }
    Ok(t.to_string())
}

/// Validates an optional URL field. An empty value is accepted and returned as an empty string.
pub fn validate_url(url: &str, locale: &str) -> Result<String, String> {
    let t = url.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    if !t.starts_with("http://") && !t.starts_with("https://") {
        return Err(rust_i18n::t!("validation.url_scheme", locale = locale).to_string());
    }
    if t.len() > MAX_URL_LEN {
        return Err(rust_i18n::t!("validation.url_too_long", locale = locale).to_string());
    }
    Ok(t.to_string())
}

/// Kept in sync with the `validation.url_too_long` message.
const MAX_URL_LEN: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_text_field() {
        assert_eq!(
            validate_text_field("  Title  ", "Title", 100, true, "en-US").unwrap(),
            "Title"
        );
        assert_eq!(
            validate_text_field("", "Title", 100, true, "en-US").unwrap_err(),
            "Title is required."
        );
        assert_eq!(
            validate_text_field("", "Title", 100, false, "en-US").unwrap(),
            ""
        );
        assert_eq!(
            validate_text_field("abc", "Title", 2, false, "en-US").unwrap_err(),
            "Title is too long (max 2 characters)."
        );
    }

    #[test]
    fn test_validate_text_field_is_localized() {
        let err = validate_text_field("", "Otsikko", 100, true, "fi-FI").unwrap_err();
        assert!(err.starts_with("Otsikko"), "unexpected message: {err}");
        assert!(err.contains("pakollinen"), "unexpected message: {err}");
    }

    #[test]
    fn test_validate_url() {
        assert_eq!(validate_url("", "en-US").unwrap(), "");
        assert_eq!(
            validate_url(" https://example.com ", "en-US").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            validate_url("example.com", "en-US").unwrap_err(),
            "URL must start with http:// or https://"
        );
        assert!(
            validate_url(
                &format!("https://example.com/{}", "a".repeat(MAX_URL_LEN)),
                "en-US"
            )
            .is_err()
        );
    }

    #[test]
    fn test_validate_url_is_localized() {
        let err = validate_url("example.com", "fi-FI").unwrap_err();
        assert!(
            err.contains("Verkko-osoitteen"),
            "unexpected message: {err}"
        );
    }
}
