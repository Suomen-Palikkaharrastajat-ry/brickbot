//! Guards over the locale bundles in `locales/`.
//!
//! `rust_i18n::i18n!` is configured with `fallback = "en-US"`, so a key that exists in only one
//! bundle fails silently: Finnish users are served English instead of an error. These tests turn
//! that into a build failure, and also catch placeholder drift between the two translations of
//! the same message.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    /// Every locale bundle the bot ships. Keep in sync with `locales/`.
    const LOCALES: [&str; 2] = ["en-US", "fi-FI"];

    fn flatten(prefix: &str, value: &serde_yaml::Value, out: &mut BTreeMap<String, String>) {
        match value {
            serde_yaml::Value::Mapping(map) => {
                for (k, v) in map {
                    let key = k.as_str().unwrap_or_default();
                    let path = if prefix.is_empty() {
                        key.to_string()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    flatten(&path, v, out);
                }
            }
            other => {
                out.insert(
                    prefix.to_string(),
                    other.as_str().unwrap_or_default().to_string(),
                );
            }
        }
    }

    fn bundle(locale: &str) -> BTreeMap<String, String> {
        let path = format!("{}/locales/{locale}.yml", env!("CARGO_MANIFEST_DIR"));
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&raw).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

        let mut out = BTreeMap::new();
        flatten("", &parsed, &mut out);
        out
    }

    /// The `%{name}` interpolations used by a message.
    fn placeholders(message: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = message;
        while let Some(start) = rest.find("%{") {
            rest = &rest[start + 2..];
            if let Some(end) = rest.find('}') {
                found.insert(rest[..end].trim().to_string());
                rest = &rest[end + 1..];
            } else {
                break;
            }
        }
        found
    }

    #[test]
    fn test_locales_define_the_same_keys() {
        let reference = bundle(LOCALES[0]);

        for locale in &LOCALES[1..] {
            let other = bundle(locale);

            let missing: Vec<_> = reference
                .keys()
                .filter(|k| !other.contains_key(*k))
                .collect();
            assert!(
                missing.is_empty(),
                "{locale} is missing keys present in {}: {missing:?}",
                LOCALES[0]
            );

            let extra: Vec<_> = other
                .keys()
                .filter(|k| !reference.contains_key(*k))
                .collect();
            assert!(
                extra.is_empty(),
                "{locale} defines keys missing from {}: {extra:?}",
                LOCALES[0]
            );
        }
    }

    #[test]
    fn test_locales_use_the_same_placeholders() {
        let reference = bundle(LOCALES[0]);

        for locale in &LOCALES[1..] {
            for (key, translated) in bundle(locale) {
                let Some(original) = reference.get(&key) else {
                    continue; // Reported by the key-parity test.
                };
                assert_eq!(
                    placeholders(original),
                    placeholders(&translated),
                    "placeholders differ for '{key}' between {} and {locale}",
                    LOCALES[0]
                );
            }
        }
    }

    #[test]
    fn test_locales_have_no_blank_translations() {
        for locale in LOCALES {
            for (key, value) in bundle(locale) {
                assert!(!value.trim().is_empty(), "{locale} has an empty '{key}'");
            }
        }
    }
}
