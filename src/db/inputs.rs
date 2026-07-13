use sqlx::SqlitePool;

pub async fn insert_input_submission(
    db: &SqlitePool,
    id: &str,
    user_id: &str,
    channel_id: &str,
    zulip_stream: &str,
    zulip_topic: &str,
    payload_str: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO inputs (id, source_user_id, discord_channel_id, zulip_stream, zulip_topic, payload_json, status, ttl) VALUES (?, ?, ?, ?, ?, ?, 'pending', datetime('now', '+7 days'))"
    )
    .bind(id)
    .bind(user_id)
    .bind(channel_id)
    .bind(zulip_stream)
    .bind(zulip_topic)
    .bind(payload_str)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn insert_draft_submission(
    db: &SqlitePool,
    id: &str,
    user_id: &str,
    channel_id: &str,
    payload_str: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO inputs (id, source_user_id, discord_channel_id, zulip_stream, zulip_topic, payload_json, status, ttl) VALUES (?, ?, ?, '', '', ?, 'draft', datetime('now', '+7 days'))"
    )
    .bind(id)
    .bind(user_id)
    .bind(channel_id)
    .bind(payload_str)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_draft_submission(db: &SqlitePool, id: &str) -> Result<String, sqlx::Error> {
    let row: (String,) = sqlx::query_as("SELECT payload_json FROM inputs WHERE id = ?")
        .bind(id)
        .fetch_one(db)
        .await?;
    Ok(row.0)
}

pub async fn get_input_moderation_action(
    db: &SqlitePool,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT moderation_action FROM inputs WHERE id = ?")
            .bind(id)
            .fetch_optional(db)
            .await?;
    Ok(row.and_then(|r| r.0))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_event_submission_transaction(
    db: &SqlitePool,
    input_id: &str,
    user_id: &str,
    channel_id: &str,
    zulip_stream: &str,
    zulip_topic: &str,
    payload_str: &str,
    outbox_id: &str,
    outbox_body: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO inputs (id, source_user_id, discord_channel_id, zulip_stream, zulip_topic, payload_json, status, ttl) VALUES (?, ?, ?, ?, ?, ?, 'queued', datetime('now', '+7 days'))"
    )
    .bind(input_id)
    .bind(user_id)
    .bind(channel_id)
    .bind(zulip_stream)
    .bind(zulip_topic)
    .bind(payload_str)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO submission_notifications (id, input_id, kind, body, status, attempt_count, created_at) VALUES (?, ?, 'zulip_post', ?, 'queued', 0, CURRENT_TIMESTAMP)"
    )
    .bind(outbox_id)
    .bind(input_id)
    .bind(outbox_body)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn get_drafting_payload(
    db: &SqlitePool,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT payload_json FROM inputs WHERE id = ? AND status = 'drafting'")
            .bind(id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(s,)| s))
}

pub async fn delete_input(db: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM inputs WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn get_pending_input_by_zulip_topic(
    db: &SqlitePool,
    zulip_topic: &str,
    zulip_stream: Option<&str>,
) -> Result<Option<(String, String, String, String)>, sqlx::Error> {
    let row: Option<(String, String, String, String)> = if let Some(stream) = zulip_stream {
        sqlx::query_as(
            "SELECT id, source_user_id, discord_channel_id, payload_json FROM inputs WHERE status IN ('pending', 'queued') AND zulip_topic = ? AND zulip_stream = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(zulip_topic)
        .bind(stream)
        .fetch_optional(db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, source_user_id, discord_channel_id, payload_json FROM inputs WHERE status IN ('pending', 'queued') AND zulip_topic = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(zulip_topic)
        .fetch_optional(db)
        .await?
    };
    Ok(row)
}

pub async fn approve_input_submission(
    db: &SqlitePool,
    id: &str,
    payload_str: &str,
    moderator_email: &str,
    message_id: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE inputs SET status = 'approved', payload_json = ?, moderated_by = ?, moderated_at = CURRENT_TIMESTAMP, moderation_action = 'approve', moderation_message_id = ? WHERE id = ? AND status IN ('pending', 'queued')"
    )
    .bind(payload_str)
    .bind(moderator_email)
    .bind(message_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

pub async fn mark_input_answered_by_zulip_topic(
    db: &SqlitePool,
    zulip_topic: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE inputs SET status = 'answered' WHERE zulip_topic = ?")
        .bind(zulip_topic)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn reject_input_submission(
    db: &SqlitePool,
    id: &str,
    moderator_email: &str,
    message_id: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE inputs SET status = 'rejected', moderated_by = ?, moderated_at = CURRENT_TIMESTAMP, moderation_action = 'reject', moderation_message_id = ? WHERE id = ? AND status IN ('pending', 'queued')"
    )
    .bind(moderator_email)
    .bind(message_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

pub async fn update_input_zulip_topic(
    db: &SqlitePool,
    id: &str,
    zulip_topic: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE inputs SET zulip_topic = ? WHERE id = ?")
        .bind(zulip_topic)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn update_input_payload(
    db: &SqlitePool,
    id: &str,
    payload_str: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE inputs SET payload_json = ? WHERE id = ?")
        .bind(payload_str)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn update_input_status(
    db: &SqlitePool,
    id: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE inputs SET status = ? WHERE id = ?")
        .bind(status)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn get_input_topic_by_discord_message_id(
    db: &SqlitePool,
    discord_message_id: &str,
) -> Result<Option<(String, String, String)>, sqlx::Error> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT i.zulip_topic, i.zulip_stream, i.payload_json FROM inputs i JOIN submission_notifications n ON i.id = n.input_id WHERE n.discord_message_id = ? AND i.zulip_topic != ''"
    )
    .bind(discord_message_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn get_latest_active_input_topic_for_user(
    db: &SqlitePool,
    user_id: &str,
) -> Result<Option<(String, String)>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT zulip_topic, zulip_stream FROM inputs WHERE source_user_id = ? AND status IN ('pending', 'queued') AND zulip_topic != '' ORDER BY id DESC LIMIT 1"
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn insert_drafting_input(
    db: &SqlitePool,
    id: &str,
    payload_str: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO inputs (id, status, payload_json, ttl) VALUES (?, 'drafting', ?, datetime('now', '+1 hour'))",
    )
    .bind(id)
    .bind(payload_str)
    .execute(db)
    .await?;
    Ok(())
}
