use super::truncate_str;
use sqlx::SqlitePool;

#[allow(clippy::too_many_arguments)]
pub async fn log_ambient_detection(
    db: &SqlitePool,
    content: Option<&str>,
    topic: &str,
    confidence: f64,
    guild_pseudonym: &str,
    channel_pseudonym: &str,
    extracted_item_id: Option<&str>,
    retention_days: u32,
) -> Result<(), sqlx::Error> {
    let content = truncate_str(content.unwrap_or(""), 2048);

    sqlx::query("INSERT INTO ambient_logs(original_message_content, detected_topic, confidence, guild_pseudonym, channel_pseudonym, extracted_item_id, ttl) VALUES(?, ?, ?, ?, ?, ?, datetime('now', '+' || ? || ' days'))")
        .bind(content)
        .bind(topic)
        .bind(confidence)
        .bind(guild_pseudonym)
        .bind(channel_pseudonym)
        .bind(extracted_item_id)
        .bind(retention_days)
        .execute(db)
        .await?;

    Ok(())
}

pub async fn defer_ambient_cooldown(
    db: &SqlitePool,
    channel_id: i64,
    topic: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE ambient_cooldowns SET last_suggested_at = datetime('now', '+2 hours') WHERE channel_id = ? AND topic = ?"
    )
    .bind(channel_id)
    .bind(topic)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn set_user_ambient_preference(
    db: &SqlitePool,
    user_id: &str,
    ignore_all: bool,
) -> Result<(), sqlx::Error> {
    let ignore_val = i32::from(ignore_all);
    sqlx::query(
        "INSERT INTO ambient_user_preferences (user_id, ignore_all) VALUES (?, ?) ON CONFLICT(user_id) DO UPDATE SET ignore_all = ?"
    )
    .bind(user_id)
    .bind(ignore_val)
    .bind(ignore_val)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn update_user_preferred_services(
    db: &SqlitePool,
    user_id: &str,
    services: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ambient_user_preferences (user_id, preferred_services) VALUES (?, ?) ON CONFLICT(user_id) DO UPDATE SET preferred_services = ?"
    )
    .bind(user_id)
    .bind(services)
    .bind(services)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_user_preferred_services(
    db: &SqlitePool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT preferred_services FROM ambient_user_preferences WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(db)
            .await?;

    Ok(row.and_then(|(s,)| s))
}

pub async fn clear_ambient_cooldown(
    db: &SqlitePool,
    channel_id: i64,
    topic: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM ambient_cooldowns WHERE channel_id = ? AND topic = ?")
        .bind(channel_id)
        .bind(topic)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn is_user_ambient_ignored(db: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM ambient_user_preferences WHERE user_id = ? AND ignore_all = 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub async fn is_user_training_opt_out(db: &SqlitePool, user_id: &str) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM ambient_user_preferences WHERE user_id = ? AND training_opt_out = 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

pub async fn get_item_cooldown(
    db: &SqlitePool,
    channel_id: i64,
    topic: &str,
    item_id: &str,
) -> Result<Option<chrono::NaiveDateTime>, sqlx::Error> {
    let row: Option<(chrono::NaiveDateTime,)> = sqlx::query_as("SELECT last_suggested_at FROM ambient_item_cooldowns WHERE channel_id = ? AND topic = ? AND item_id = ?")
        .bind(channel_id)
        .bind(topic)
        .bind(item_id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn get_topic_cooldown(
    db: &SqlitePool,
    channel_id: i64,
    topic: &str,
) -> Result<Option<chrono::NaiveDateTime>, sqlx::Error> {
    let row: Option<(chrono::NaiveDateTime,)> = sqlx::query_as(
        "SELECT last_suggested_at FROM ambient_cooldowns WHERE channel_id = ? AND topic = ?",
    )
    .bind(channel_id)
    .bind(topic)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn set_topic_cooldown(
    db: &SqlitePool,
    channel_id: i64,
    topic: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO ambient_cooldowns (channel_id, topic, last_suggested_at, ttl) VALUES (?, ?, CURRENT_TIMESTAMP, datetime('now', '+2 days')) ON CONFLICT(channel_id, topic) DO UPDATE SET last_suggested_at = CURRENT_TIMESTAMP, ttl = datetime('now', '+2 days')")
        .bind(channel_id)
        .bind(topic)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_item_cooldown(
    db: &SqlitePool,
    channel_id: i64,
    topic: &str,
    item_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO ambient_item_cooldowns (channel_id, topic, item_id, last_suggested_at, ttl) VALUES (?, ?, ?, CURRENT_TIMESTAMP, datetime('now', '+2 days')) ON CONFLICT(channel_id, topic, item_id) DO UPDATE SET last_suggested_at = CURRENT_TIMESTAMP, ttl = datetime('now', '+2 days')")
        .bind(channel_id)
        .bind(topic)
        .bind(item_id)
        .execute(db)
        .await?;
    Ok(())
}
