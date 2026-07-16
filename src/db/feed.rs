use sqlx::SqlitePool;

pub struct FeedItem {
    pub id: String,
    pub source_title: String,
    pub item_title: String,
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    &s[..end]
}

pub async fn insert_feed_item(
    db: &SqlitePool,
    id: &str,
    source_title: &str,
    item_title: &str,
    item_description: &str,
) -> Result<(), sqlx::Error> {
    let source_title = truncate_str(source_title, 256);
    let item_title = truncate_str(item_title, 256);
    let item_description = truncate_str(item_description, 2048);

    sqlx::query(
        "INSERT INTO feed_items(id, source_title, item_title, item_description) VALUES(?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET source_title=excluded.source_title, item_title=excluded.item_title, item_description=excluded.item_description"
    )
    .bind(id)
    .bind(source_title)
    .bind(item_title)
    .bind(item_description)
    .execute(db)
    .await?;

    Ok(())
}

#[must_use]
pub fn filter_feed_items(
    rows: Vec<(String, String, String, String)>,
    set_number: &str,
    set_name: &str,
    set_theme: &str,
) -> Vec<FeedItem> {
    let number_regex = regex::Regex::new(&format!(r"\b{}\b", regex::escape(set_number))).unwrap();

    let name_tokens: Vec<String> = set_name
        .to_lowercase()
        .split_whitespace()
        .map(std::string::ToString::to_string)
        .collect();
    let theme_tokens: Vec<String> = set_theme
        .to_lowercase()
        .split_whitespace()
        .map(std::string::ToString::to_string)
        .collect();

    let mut filtered_items = Vec::new();

    for (id, source_title, item_title, item_description) in rows {
        let title_lower = item_title.to_lowercase();
        let desc_lower = item_description.to_lowercase();

        if !number_regex.is_match(&item_title) && !number_regex.is_match(&item_description) {
            continue;
        }

        let is_match = if number_regex.is_match(&item_title) {
            true
        } else {
            name_tokens.iter().chain(theme_tokens.iter()).any(|token| {
                token.len() > 3 && (title_lower.contains(token) || desc_lower.contains(token))
            })
        };

        if is_match {
            filtered_items.push(FeedItem {
                id,
                source_title,
                item_title,
            });
            if filtered_items.len() >= 5 {
                break;
            }
        }
    }

    filtered_items
}

pub async fn search_feed_items(
    db: &SqlitePool,
    set_number: &str,
    set_name: &str,
    set_theme: &str,
) -> Result<Vec<FeedItem>, sqlx::Error> {
    let search_pattern = format!("%{set_number}%");
    let rows: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT id, source_title, item_title, item_description FROM feed_items WHERE item_title LIKE ? OR item_description LIKE ? LIMIT 20")
            .bind(&search_pattern)
            .bind(&search_pattern)
            .fetch_all(db)
            .await?;

    Ok(filter_feed_items(rows, set_number, set_name, set_theme))
}

pub async fn get_last_polled_at(db: &SqlitePool, url: &str) -> Option<chrono::NaiveDateTime> {
    let row: Option<(chrono::NaiveDateTime,)> =
        sqlx::query_as("SELECT last_polled_at FROM feed_polls WHERE url = ?")
            .bind(url)
            .fetch_optional(db)
            .await
            .unwrap_or(None);
    row.map(|(dt,)| dt)
}

pub async fn mark_polled(db: &SqlitePool, url: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO feed_polls (url, last_polled_at) VALUES (?, CURRENT_TIMESTAMP) ON CONFLICT(url) DO UPDATE SET last_polled_at = CURRENT_TIMESTAMP")
        .bind(url)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_feed_items() {
        let rows = vec![
            (
                "1".to_string(),
                "Source".to_string(),
                "LEGO 77093 Review".to_string(),
                "A review of the new set.".to_string(),
            ),
            (
                "2".to_string(),
                "Source".to_string(),
                "Random Article".to_string(),
                "The number 177093 is not the set.".to_string(),
            ),
            (
                "3".to_string(),
                "Source".to_string(),
                "Other Article".to_string(),
                "This mentions 77093 but has nothing to do with it.".to_string(),
            ),
            (
                "4".to_string(),
                "Source".to_string(),
                "Zelda News".to_string(),
                "The great deku tree 77093 is amazing.".to_string(),
            ),
        ];

        let filtered = filter_feed_items(rows, "77093", "Great Deku Tree", "The Legend of Zelda");

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "1"); // Matches in title
        assert_eq!(filtered[1].id, "4"); // Matches in description AND has "great" or "deku"
    }
}
