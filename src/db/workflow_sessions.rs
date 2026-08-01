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

/// The newest live session of a kind, for one person in one channel.
///
/// Every other accessor here starts from a session id, which a slash command
/// does not have: `/build-legend` knows only who ran it and where. Returns the
/// id alongside the payload, because the caller needs it to write back.
///
/// Newest first because a person may briefly hold more than one — the ordering
/// keeps that from being ambiguous even when it happens.
pub async fn find_latest_workflow_session(
    db: &SqlitePool,
    kind: &str,
    owner_user_id: &str,
    channel_id: &str,
) -> Result<Option<(String, Value)>, sqlx::Error> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT id, payload_json FROM workflow_sessions
         WHERE kind = ? AND owner_user_id = ? AND channel_id = ?
           AND consumed_at IS NULL AND expires_at > CURRENT_TIMESTAMP
         ORDER BY created_at DESC, rowid DESC
         LIMIT 1",
    )
    .bind(kind)
    .bind(owner_user_id)
    .bind(channel_id)
    .fetch_optional(db)
    .await?;

    Ok(row.map(|(id, payload)| (id, serde_json::from_str(&payload).unwrap_or(Value::Null))))
}

/// Marks a session finished, so it stops being found.
///
/// Distinct from letting it expire: a draft that has been applied or discarded
/// is over, and leaving it live would make the *next* lookup return something
/// the builder has already dealt with.
pub async fn consume_workflow_session(
    db: &SqlitePool,
    session_id: &str,
    owner_user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE workflow_sessions SET consumed_at = CURRENT_TIMESTAMP
         WHERE id = ? AND owner_user_id = ? AND consumed_at IS NULL",
    )
    .bind(session_id)
    .bind(owner_user_id)
    .execute(db)
    .await?;
    Ok(())
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

    #[tokio::test]
    async fn test_update_and_get_session_payload() {
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

        let fetched = get_workflow_session_payload(&db, &session_id, "user1")
            .await
            .unwrap();
        assert_eq!(fetched, Some(payload));

        let updated_payload = serde_json::json!({"test": "updated"});
        update_workflow_session_payload(&db, &session_id, "user1", updated_payload.clone())
            .await
            .unwrap();

        let fetched_updated = get_workflow_session_payload(&db, &session_id, "user1")
            .await
            .unwrap();
        assert_eq!(fetched_updated, Some(updated_payload));

        let fetched_wrong_user = get_workflow_session_payload(&db, &session_id, "user2")
            .await
            .unwrap();
        assert_eq!(fetched_wrong_user, None);
    }

    /// Opens a session and returns its id.
    async fn open(db: &SqlitePool, kind: &str, user: &str, channel: &str) -> String {
        create_workflow_session(
            db,
            kind,
            user,
            "guild1",
            channel,
            serde_json::json!({ "at": channel }),
            15,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_session_is_found_by_who_opened_it_and_where() {
        // What a slash command has to work from: it knows the person and the
        // channel, never a session id.
        let db = setup_db().await;
        let id = open(&db, "build_draft", "user1", "chan1").await;

        let found = find_latest_workflow_session(&db, "build_draft", "user1", "chan1")
            .await
            .unwrap();
        assert_eq!(found.map(|(id, _)| id), Some(id));
    }

    #[tokio::test]
    async fn a_session_belonging_to_someone_else_or_elsewhere_is_not_found() {
        // A draft is private, and a build thread is shared, so both halves of
        // the key have to be honoured.
        let db = setup_db().await;
        open(&db, "build_draft", "user1", "chan1").await;

        for (kind, user, channel) in [
            ("build_draft", "user2", "chan1"),
            ("build_draft", "user1", "chan2"),
            ("submit_event", "user1", "chan1"),
        ] {
            assert!(
                find_latest_workflow_session(&db, kind, user, channel)
                    .await
                    .unwrap()
                    .is_none(),
                "({kind}, {user}, {channel}) should not have matched"
            );
        }
    }

    #[tokio::test]
    async fn the_newest_session_wins_when_someone_holds_more_than_one() {
        let db = setup_db().await;
        open(&db, "build_draft", "user1", "chan1").await;
        let second = open(&db, "build_draft", "user1", "chan1").await;

        let found = find_latest_workflow_session(&db, "build_draft", "user1", "chan1")
            .await
            .unwrap();
        assert_eq!(found.map(|(id, _)| id), Some(second));
    }

    #[tokio::test]
    async fn a_consumed_session_stops_being_found() {
        // What makes discarding or applying a draft final, rather than leaving
        // it to be picked up again by the next command.
        let db = setup_db().await;
        let id = open(&db, "build_draft", "user1", "chan1").await;

        consume_workflow_session(&db, &id, "user1").await.unwrap();
        assert!(
            find_latest_workflow_session(&db, "build_draft", "user1", "chan1")
                .await
                .unwrap()
                .is_none()
        );
        // And it can no longer be read or written by id either.
        assert!(
            get_workflow_session_payload(&db, &id, "user1")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn only_the_owner_can_consume_a_session() {
        let db = setup_db().await;
        let id = open(&db, "build_draft", "user1", "chan1").await;

        consume_workflow_session(&db, &id, "user2").await.unwrap();
        assert!(
            find_latest_workflow_session(&db, "build_draft", "user1", "chan1")
                .await
                .unwrap()
                .is_some(),
            "someone else's consume must not close it"
        );
    }

    #[tokio::test]
    async fn the_payload_comes_back_with_the_id() {
        let db = setup_db().await;
        open(&db, "build_draft", "user1", "chan1").await;

        let (_, payload) = find_latest_workflow_session(&db, "build_draft", "user1", "chan1")
            .await
            .unwrap()
            .expect("a session");
        assert_eq!(payload, serde_json::json!({ "at": "chan1" }));
    }
}
