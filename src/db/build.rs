//! Storage for collaborative `/build` sessions.
//!
//! Only the operation log is persisted; the model is a fold over it. That
//! keeps writes O(1) per command, gives "who added the roof?" for free, and
//! means the derived model can change shape without a data migration.
//!
//! Unlike `workflow_sessions`, nothing here is scoped to an owner. A build
//! session is collaborative: anyone who can see the thread can edit the model,
//! and Discord already enforces who that is.

#![allow(clippy::doc_markdown)]

use crate::build::ops::{AppliedOp, History, Op};
use sqlx::SqlitePool;

/// A live build session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSession {
    pub id: String,
    pub guild_id: String,
    pub channel_id: String,
    pub owner_user_id: String,
    pub model_name: String,
}

/// Opens a session for a thread.
///
/// The unique partial index means a second live session in the same thread is
/// rejected by the database rather than by a check that could race.
pub async fn create_build_session(
    db: &SqlitePool,
    guild_id: &str,
    channel_id: &str,
    owner_user_id: &str,
    model_name: &str,
    expires_in_hours: u32,
) -> Result<BuildSession, sqlx::Error> {
    // Truncated so it fits comfortably inside a component custom_id.
    let id = uuid::Uuid::new_v4().as_simple().to_string()[..12].to_string();

    sqlx::query(
        "INSERT INTO build_sessions (id, guild_id, channel_id, owner_user_id, model_name, expires_at)
         VALUES (?, ?, ?, ?, ?, datetime('now', '+' || ? || ' hours'))",
    )
    .bind(&id)
    .bind(guild_id)
    .bind(channel_id)
    .bind(owner_user_id)
    .bind(model_name)
    .bind(expires_in_hours)
    .execute(db)
    .await?;

    Ok(BuildSession {
        id,
        guild_id: guild_id.to_string(),
        channel_id: channel_id.to_string(),
        owner_user_id: owner_user_id.to_string(),
        model_name: model_name.to_string(),
    })
}

/// Finds the live session for a thread, if there is one.
///
/// This runs for every message in a build thread, which is why the
/// `channel_id` index exists.
pub async fn get_active_session_by_channel(
    db: &SqlitePool,
    channel_id: &str,
) -> Result<Option<BuildSession>, sqlx::Error> {
    let row: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, guild_id, channel_id, owner_user_id, model_name
         FROM build_sessions
         WHERE channel_id = ? AND status = 'active' AND expires_at > CURRENT_TIMESTAMP",
    )
    .bind(channel_id)
    .fetch_optional(db)
    .await?;

    Ok(row.map(
        |(id, guild_id, channel_id, owner_user_id, model_name)| BuildSession {
            id,
            guild_id,
            channel_id,
            owner_user_id,
            model_name,
        },
    ))
}

/// Appends one operation to a session's log.
pub async fn append_revision(
    db: &SqlitePool,
    session_id: &str,
    n: u32,
    author_user_id: &str,
    op: &Op,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_string(op).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query(
        "INSERT OR REPLACE INTO build_revisions (session_id, n, author_user_id, command_json)
         VALUES (?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(n)
    .bind(author_user_id)
    .bind(json)
    .execute(db)
    .await?;
    Ok(())
}

/// Discards every revision from `n` onwards.
///
/// Called when a new command arrives after an undo, which invalidates the redo
/// tail — the same semantics as any editor.
pub async fn truncate_from(db: &SqlitePool, session_id: &str, n: u32) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM build_revisions WHERE session_id = ? AND n >= ?")
        .bind(session_id)
        .bind(n)
        .execute(db)
        .await?;
    Ok(())
}

/// Marks a single revision skipped, for `Undo Alice's last edit.`
///
/// The row stays so that revision indices — and therefore placement ids —
/// remain stable.
pub async fn mark_skipped(
    db: &SqlitePool,
    session_id: &str,
    n: u32,
    skipped: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE build_revisions SET skipped = ? WHERE session_id = ? AND n = ?")
        .bind(i32::from(skipped))
        .bind(session_id)
        .bind(n)
        .execute(db)
        .await?;
    Ok(())
}

/// Loads a session's whole history.
///
/// Revisions whose stored operation no longer deserialises are skipped with a
/// warning rather than failing the load: a session must survive an operation
/// shape changing between releases.
pub async fn load_history(
    db: &SqlitePool,
    session_id: &str,
    model_name: &str,
) -> Result<History, sqlx::Error> {
    let rows: Vec<(i64, String, String, i64, String)> = sqlx::query_as(
        "SELECT n, author_user_id, command_json, skipped, created_at
         FROM build_revisions WHERE session_id = ? ORDER BY n ASC",
    )
    .bind(session_id)
    .fetch_all(db)
    .await?;

    let mut history = History::new(model_name);
    for (n, author, json, skipped, created_at) in rows {
        match serde_json::from_str::<Op>(&json) {
            Ok(op) => history.ops.push(AppliedOp {
                op,
                author,
                at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map_or_else(|_| chrono::Utc::now(), |t| t.with_timezone(&chrono::Utc)),
                skipped: skipped != 0,
            }),
            Err(e) => {
                tracing::warn!("build: revision {n} of session {session_id} is unreadable: {e}");
            }
        }
    }
    history.cursor = history.ops.len();
    Ok(history)
}

/// Records how many revisions are currently in effect, so undo and redo
/// survive a restart.
pub async fn set_cursor(
    db: &SqlitePool,
    session_id: &str,
    cursor: usize,
) -> Result<(), sqlx::Error> {
    // The cursor is expressed by deleting nothing and instead marking the tail
    // skipped, so a restart reproduces exactly what the thread last saw.
    sqlx::query("UPDATE build_revisions SET skipped = (n >= ?) WHERE session_id = ? AND n >= ?")
        .bind(i64::try_from(cursor).unwrap_or(i64::MAX))
        .bind(session_id)
        .bind(i64::try_from(cursor).unwrap_or(i64::MAX))
        .execute(db)
        .await?;
    Ok(())
}

/// Closes a session, freeing its thread for a new one.
pub async fn close_session(db: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE build_sessions SET status = 'closed' WHERE id = ?")
        .bind(session_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Extends a session's life, so an active build does not expire mid-sentence.
pub async fn touch_session(
    db: &SqlitePool,
    session_id: &str,
    expires_in_hours: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE build_sessions SET expires_at = datetime('now', '+' || ? || ' hours') WHERE id = ?",
    )
    .bind(expires_in_hours)
    .bind(session_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Deletes expired sessions and their revisions.
pub async fn cleanup_expired_sessions(db: &SqlitePool) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "DELETE FROM build_revisions WHERE session_id IN
         (SELECT id FROM build_sessions WHERE expires_at < CURRENT_TIMESTAMP)",
    )
    .execute(db)
    .await?;

    let result = sqlx::query("DELETE FROM build_sessions WHERE expires_at < CURRENT_TIMESTAMP")
        .execute(db)
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::model::{PartKind, PartRef};
    use crate::build::units::{IVec3, Orientation};

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Run the real migrations, so the migration itself is under test.
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn add_op(color: u16, pos: IVec3) -> Op {
        Op::Add {
            part: PartRef::new("3001", "Brick 2 x 4", (2, 4), PartKind::Brick),
            color,
            pos,
            orientation: Orientation::DEFAULT,
            repeat: None,
            group: None,
        }
    }

    async fn session(db: &SqlitePool) -> BuildSession {
        create_build_session(db, "guild1", "thread1", "alice", "Castle", 24)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn finds_an_active_session_by_channel() {
        let db = setup_db().await;
        let created = session(&db).await;

        let found = get_active_session_by_channel(&db, "thread1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found, created);
        assert_eq!(found.model_name, "Castle");

        assert!(get_active_session_by_channel(&db, "other")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn only_one_active_session_per_channel() {
        let db = setup_db().await;
        session(&db).await;

        // The partial unique index rejects a second live session, so two
        // people running /build at once cannot both win.
        let second = create_build_session(&db, "guild1", "thread1", "bob", "Other", 24).await;
        assert!(second.is_err());
    }

    #[tokio::test]
    async fn closing_a_session_frees_the_channel() {
        let db = setup_db().await;
        let first = session(&db).await;
        close_session(&db, &first.id).await.unwrap();

        assert!(get_active_session_by_channel(&db, "thread1")
            .await
            .unwrap()
            .is_none());
        // A new session can now be opened in the same thread.
        assert!(
            create_build_session(&db, "guild1", "thread1", "bob", "Second", 24)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn revisions_round_trip_through_the_history() {
        let db = setup_db().await;
        let s = session(&db).await;

        append_revision(&db, &s.id, 0, "alice", &add_op(4, IVec3::ZERO))
            .await
            .unwrap();
        append_revision(&db, &s.id, 1, "bob", &add_op(14, IVec3::new(0, -24, 0)))
            .await
            .unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.ops.len(), 2);
        assert_eq!(history.cursor, 2);

        let model = history.materialize();
        assert_eq!(model.name, "Castle");
        assert_eq!(model.len(), 2);
        assert_eq!(model.placements[0].color, 4);
        assert_eq!(model.placements[1].color, 14);
    }

    #[tokio::test]
    async fn the_history_remembers_who_did_what() {
        let db = setup_db().await;
        let s = session(&db).await;
        append_revision(&db, &s.id, 0, "alice", &add_op(4, IVec3::ZERO))
            .await
            .unwrap();
        append_revision(&db, &s.id, 1, "bob", &add_op(14, IVec3::new(100, 0, 0)))
            .await
            .unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.ops[0].author, "alice");
        assert_eq!(history.ops[1].author, "bob");
    }

    #[tokio::test]
    async fn any_member_can_load_the_session() {
        // The whole point of not reusing workflow_sessions: the lookup is by
        // channel, never by owner, so everyone in the thread sees the same
        // model.
        let db = setup_db().await;
        let owner = session(&db).await;
        append_revision(&db, &owner.id, 0, "alice", &add_op(4, IVec3::ZERO))
            .await
            .unwrap();

        let seen_by_anyone = get_active_session_by_channel(&db, "thread1")
            .await
            .unwrap()
            .unwrap();
        let history = load_history(&db, &seen_by_anyone.id, &seen_by_anyone.model_name)
            .await
            .unwrap();
        assert_eq!(history.materialize().len(), 1);
    }

    #[tokio::test]
    async fn a_skipped_revision_is_left_out_of_the_model() {
        let db = setup_db().await;
        let s = session(&db).await;
        append_revision(&db, &s.id, 0, "alice", &add_op(4, IVec3::ZERO))
            .await
            .unwrap();
        append_revision(&db, &s.id, 1, "bob", &add_op(14, IVec3::new(100, 0, 0)))
            .await
            .unwrap();

        mark_skipped(&db, &s.id, 0, true).await.unwrap();
        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        let model = history.materialize();

        assert_eq!(model.len(), 1);
        // Bob's brick keeps revision 1, so its id is unchanged.
        assert!(model.contains(crate::build::PlacementId::new(1, 0)));
    }

    #[tokio::test]
    async fn truncating_discards_the_redo_tail() {
        let db = setup_db().await;
        let s = session(&db).await;
        for n in 0..4 {
            let x = i32::try_from(n).unwrap() * 100;
            append_revision(&db, &s.id, n, "alice", &add_op(4, IVec3::new(x, 0, 0)))
                .await
                .unwrap();
        }

        truncate_from(&db, &s.id, 2).await.unwrap();
        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.ops.len(), 2);
    }

    #[tokio::test]
    async fn an_unreadable_revision_does_not_sink_the_session() {
        let db = setup_db().await;
        let s = session(&db).await;
        append_revision(&db, &s.id, 0, "alice", &add_op(4, IVec3::ZERO))
            .await
            .unwrap();
        // Simulate an operation written by a future release.
        sqlx::query(
            "INSERT INTO build_revisions (session_id, n, author_user_id, command_json)
             VALUES (?, 1, 'bob', '{\"Teleport\":{}}')",
        )
        .bind(&s.id)
        .execute(&db)
        .await
        .unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.ops.len(), 1, "the readable revision survives");
        assert_eq!(history.materialize().len(), 1);
    }

    #[tokio::test]
    async fn expired_sessions_and_their_revisions_are_cleaned_up() {
        let db = setup_db().await;
        let live = session(&db).await;
        append_revision(&db, &live.id, 0, "alice", &add_op(4, IVec3::ZERO))
            .await
            .unwrap();

        // A session that expired an hour ago.
        sqlx::query(
            "INSERT INTO build_sessions (id, guild_id, channel_id, owner_user_id, model_name, expires_at)
             VALUES ('old', 'guild1', 'thread2', 'bob', 'Old', datetime('now', '-1 hours'))",
        )
        .execute(&db)
        .await
        .unwrap();
        append_revision(&db, "old", 0, "bob", &add_op(4, IVec3::ZERO))
            .await
            .unwrap();

        let removed = cleanup_expired_sessions(&db).await.unwrap();
        assert_eq!(removed, 1);
        assert!(get_active_session_by_channel(&db, "thread2")
            .await
            .unwrap()
            .is_none());
        // The live session and its revisions are untouched.
        assert!(get_active_session_by_channel(&db, "thread1")
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            load_history(&db, &live.id, "Castle")
                .await
                .unwrap()
                .ops
                .len(),
            1
        );
        assert!(load_history(&db, "old", "Old")
            .await
            .unwrap()
            .ops
            .is_empty());
    }

    #[tokio::test]
    async fn an_expired_session_is_not_returned_even_before_cleanup() {
        let db = setup_db().await;
        sqlx::query(
            "INSERT INTO build_sessions (id, guild_id, channel_id, owner_user_id, model_name, expires_at)
             VALUES ('old', 'guild1', 'thread3', 'bob', 'Old', datetime('now', '-1 hours'))",
        )
        .execute(&db)
        .await
        .unwrap();

        assert!(get_active_session_by_channel(&db, "thread3")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn touching_a_session_extends_its_life() {
        let db = setup_db().await;
        sqlx::query(
            "INSERT INTO build_sessions (id, guild_id, channel_id, owner_user_id, model_name, expires_at)
             VALUES ('soon', 'guild1', 'thread4', 'bob', 'Soon', datetime('now', '-1 hours'))",
        )
        .execute(&db)
        .await
        .unwrap();
        assert!(get_active_session_by_channel(&db, "thread4")
            .await
            .unwrap()
            .is_none());

        touch_session(&db, "soon", 24).await.unwrap();
        assert!(get_active_session_by_channel(&db, "thread4")
            .await
            .unwrap()
            .is_some());
    }
}
