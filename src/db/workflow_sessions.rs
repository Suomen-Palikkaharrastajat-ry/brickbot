use serde_json::Value;
use sqlx::SqlitePool;

pub async fn create_workflow_session(
    db: &SqlitePool,
    kind: &str,
    owner_user_id: &str,
    guild_id: &str,
    channel_id: &str,
    payload: Value,
    expires_in_mins: u32,
) -> Result<String, sqlx::Error> {
    let session_id = uuid::Uuid::new_v4().as_simple().to_string()[..12].to_string();
    let payload_str = payload.to_string();

    sqlx::query("INSERT INTO workflow_sessions (id, kind, owner_user_id, guild_id, channel_id, payload_json, expires_at) VALUES (?, ?, ?, ?, ?, ?, datetime('now', '+' || ? || ' minutes'))")
        .bind(&session_id)
        .bind(kind)
        .bind(owner_user_id)
        .bind(guild_id)
        .bind(channel_id)
        .bind(payload_str)
        .bind(expires_in_mins)
        .execute(db).await?;

    Ok(session_id)
}

pub async fn update_workflow_session_payload(
    db: &SqlitePool,
    session_id: &str,
    user_id: &str,
    payload: Value,
) -> Result<(), sqlx::Error> {
    let payload_str = payload.to_string();
    sqlx::query("UPDATE workflow_sessions SET payload_json = ? WHERE id = ? AND owner_user_id = ? AND consumed_at IS NULL AND expires_at > CURRENT_TIMESTAMP")
        .bind(payload_str)
        .bind(session_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn get_workflow_session_payload(
    db: &SqlitePool,
    session_id: &str,
    user_id: &str,
) -> Result<Option<Value>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT payload_json FROM workflow_sessions WHERE id = ? AND owner_user_id = ? AND consumed_at IS NULL AND expires_at > CURRENT_TIMESTAMP"
    )
    .bind(session_id)
    .bind(user_id)
    .fetch_optional(db).await?;

    if let Some((payload,)) = row {
        return Ok(Some(serde_json::from_str(&payload).unwrap_or(Value::Null)));
    }
    Ok(None)
}

pub async fn authorize_and_consume_session(
    db: &SqlitePool,
    session_id: &str,
    expected_kind: &str,
    user_id: &str,
    guild_id: &str,
) -> Result<Option<Value>, sqlx::Error> {
    let mut tx = db.begin().await?;

    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT payload_json, owner_user_id, guild_id, kind FROM workflow_sessions WHERE id = ? AND consumed_at IS NULL AND expires_at > CURRENT_TIMESTAMP"
    )
    .bind(session_id)
    .fetch_optional(&mut *tx).await?;

    if let Some((payload, owner_id, guild, kind)) = row {
        if owner_id == user_id && guild == guild_id && kind == expected_kind {
            sqlx::query(
                "UPDATE workflow_sessions SET consumed_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;

            return Ok(Some(serde_json::from_str(&payload).unwrap_or(Value::Null)));
        }
    }

    tx.rollback().await?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE workflow_sessions (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                owner_user_id TEXT NOT NULL,
                guild_id TEXT NOT NULL,
                channel_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                expires_at DATETIME NOT NULL,
                consumed_at DATETIME
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn test_create_and_consume_session() {
        let db = setup_db().await;
        let payload = serde_json::json!({"test": "data"});
        let session_id = create_workflow_session(
            &db,
            "test_kind",
            "user1",
            "guild1",
            "chan1",
            payload.clone(),
            15,
        )
        .await
        .unwrap();

        let consumed =
            authorize_and_consume_session(&db, &session_id, "test_kind", "user1", "guild1")
                .await
                .unwrap();
        assert_eq!(consumed, Some(payload));

        let consumed2 =
            authorize_and_consume_session(&db, &session_id, "test_kind", "user1", "guild1")
                .await
                .unwrap();
        assert_eq!(consumed2, None);
    }

    #[tokio::test]
    async fn test_authorize_invalid_owner() {
        let db = setup_db().await;
        let payload = serde_json::json!({"test": "data"});
        let session_id = create_workflow_session(
            &db,
            "test_kind",
            "user1",
            "guild1",
            "chan1",
            payload.clone(),
            15,
        )
        .await
        .unwrap();

        let consumed =
            authorize_and_consume_session(&db, &session_id, "test_kind", "hacker", "guild1")
                .await
                .unwrap();
        assert_eq!(consumed, None);
    }
}
