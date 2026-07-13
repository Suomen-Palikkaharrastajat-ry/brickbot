use sqlx::SqlitePool;

pub async fn insert_notification(
    db: &SqlitePool,
    id: &str,
    input_id: &str,
    kind: &str,
    body: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO submission_notifications (id, input_id, kind, body) VALUES (?, ?, ?, ?)",
    )
    .bind(id)
    .bind(input_id)
    .bind(kind)
    .bind(body)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn mark_notification_sent(
    db: &SqlitePool,
    id: &str,
    discord_message_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE submission_notifications SET status = 'sent', sent_at = CURRENT_TIMESTAMP, discord_message_id = ? WHERE id = ?")
        .bind(discord_message_id)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn mark_notification_dm_failed(db: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE submission_notifications SET status = 'dm_failed', attempt_count = 1 WHERE id = ?",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get_unread_notifications_for_user(
    db: &SqlitePool,
    user_id: &str,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT n.id, n.body FROM submission_notifications n
         JOIN inputs i ON n.input_id = i.id
         WHERE i.source_user_id = ? AND n.status IN ('queued', 'dm_failed')",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn mark_notification_read(db: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE submission_notifications SET status = 'read' WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn get_queued_notifications(
    db: &SqlitePool,
    limit: u32,
) -> Result<Vec<(String, String, String, String, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, input_id, kind, body, attempt_count FROM submission_notifications WHERE status = 'queued' LIMIT ?"
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn mark_notification_failed(db: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE submission_notifications SET status = 'failed', attempt_count = attempt_count + 1 WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn update_notification_status(
    db: &SqlitePool,
    id: &str,
    status: &str,
    increment_attempt: bool,
    error_code: Option<i32>,
) -> Result<(), sqlx::Error> {
    if increment_attempt {
        sqlx::query("UPDATE submission_notifications SET status = ?, attempt_count = attempt_count + 1, last_error_code = ? WHERE id = ?")
            .bind(status)
            .bind(error_code)
            .bind(id)
            .execute(db)
            .await?;
    } else if status == "sent" {
        sqlx::query("UPDATE submission_notifications SET status = 'sent', sent_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(id)
            .execute(db)
            .await?;
    } else {
        sqlx::query("UPDATE submission_notifications SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(())
}
