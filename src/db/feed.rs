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

pub async fn search_feed_items(db: &SqlitePool, query: &str) -> Result<Vec<FeedItem>, sqlx::Error> {
    let search_pattern = format!("%{query}%");
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, source_title, item_title FROM feed_items WHERE item_title LIKE ? OR item_description LIKE ? LIMIT 5")
            .bind(&search_pattern)
            .bind(&search_pattern)
            .fetch_all(db)
            .await?;

    Ok(rows
        .into_iter()
        .map(|(id, source_title, item_title)| FeedItem {
            id,
            source_title,
            item_title,
        })
        .collect())
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
