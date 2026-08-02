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
use crate::build::units::MaxModelSize;
use sqlx::SqlitePool;

/// A live build session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSession {
    pub id: String,
    pub guild_id: String,
    pub channel_id: String,
    pub owner_user_id: String,
    pub model_name: String,
    /// How many revisions are currently in effect, i.e. where undo and redo
    /// have left the session. `None` means every revision is live.
    pub cursor: Option<usize>,
    /// A sentence waiting on a part the catalogue could not resolve, to be
    /// retried once the builder picks one from the dropdown.
    pub pending_sentence: Option<String>,
    /// The `size` this session was opened with, if the builder gave one.
    /// Set together or not at all; `None` on any of the three means no
    /// override, so the session falls back to the guild's config default.
    pub max_size_x: Option<u32>,
    pub max_size_y: Option<u32>,
    pub max_size_z: Option<u32>,
}

impl BuildSession {
    /// This session's size override, if it was given one at creation.
    #[must_use]
    pub fn max_size(&self) -> Option<MaxModelSize> {
        Some(MaxModelSize {
            x: self.max_size_x?,
            y: self.max_size_y?,
            z: self.max_size_z?,
        })
    }
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
    max_size: Option<MaxModelSize>,
) -> Result<BuildSession, sqlx::Error> {
    // Truncated so it fits comfortably inside a component custom_id.
    let id = uuid::Uuid::new_v4().as_simple().to_string()[..12].to_string();

    sqlx::query(
        "INSERT INTO build_sessions
             (id, guild_id, channel_id, owner_user_id, model_name, expires_at,
              max_size_x, max_size_y, max_size_z)
         VALUES (?, ?, ?, ?, ?, datetime('now', '+' || ? || ' hours'), ?, ?, ?)",
    )
    .bind(&id)
    .bind(guild_id)
    .bind(channel_id)
    .bind(owner_user_id)
    .bind(model_name)
    .bind(expires_in_hours)
    .bind(max_size.map(|s| i64::from(s.x)))
    .bind(max_size.map(|s| i64::from(s.y)))
    .bind(max_size.map(|s| i64::from(s.z)))
    .execute(db)
    .await?;

    Ok(BuildSession {
        id,
        guild_id: guild_id.to_string(),
        channel_id: channel_id.to_string(),
        owner_user_id: owner_user_id.to_string(),
        model_name: model_name.to_string(),
        cursor: None,
        pending_sentence: None,
        max_size_x: max_size.map(|s| s.x),
        max_size_y: max_size.map(|s| s.y),
        max_size_z: max_size.map(|s| s.z),
    })
}

/// One `build_sessions` row as selected, before it becomes a [`BuildSession`]:
/// id, guild, channel, owner, model name, cursor, pending sentence, size cap
/// on X, Y and Z.
type SessionRow = (
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// Finds the live session for a thread, if there is one.
///
/// This runs for every message in a build thread, which is why the
/// `channel_id` index exists.
pub async fn get_active_session_by_channel(
    db: &SqlitePool,
    channel_id: &str,
) -> Result<Option<BuildSession>, sqlx::Error> {
    let row: Option<SessionRow> = sqlx::query_as(
        "SELECT id, guild_id, channel_id, owner_user_id, model_name, cursor, pending_sentence,
                max_size_x, max_size_y, max_size_z
         FROM build_sessions
         WHERE channel_id = ? AND status = 'active' AND expires_at > CURRENT_TIMESTAMP",
    )
    .bind(channel_id)
    .fetch_optional(db)
    .await?;

    Ok(row.map(
        |(
            id,
            guild_id,
            channel_id,
            owner_user_id,
            model_name,
            cursor,
            pending_sentence,
            max_size_x,
            max_size_y,
            max_size_z,
        )| {
            BuildSession {
                id,
                guild_id,
                channel_id,
                owner_user_id,
                model_name,
                cursor: cursor.and_then(|c| usize::try_from(c).ok()),
                pending_sentence,
                max_size_x: max_size_x.and_then(|n| u32::try_from(n).ok()),
                max_size_y: max_size_y.and_then(|n| u32::try_from(n).ok()),
                max_size_z: max_size_z.and_then(|n| u32::try_from(n).ok()),
            }
        },
    ))
}

/// Appends one operation to a session's log.
pub async fn append_revision<'e, E>(
    db: E,
    session_id: &str,
    n: u32,
    author_user_id: &str,
    op: &Op,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
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
pub async fn truncate_from<'e, E>(db: E, session_id: &str, n: u32) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
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
pub async fn mark_skipped<'e, E>(
    db: E,
    session_id: &str,
    n: u32,
    skipped: bool,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query("UPDATE build_revisions SET skipped = ? WHERE session_id = ? AND n = ?")
        .bind(i32::from(skipped))
        .bind(session_id)
        .bind(n)
        .execute(db)
        .await?;
    Ok(())
}

/// Loads a session's whole history, including where undo and redo left it.
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

    let stored_cursor: Option<i64> =
        sqlx::query_scalar("SELECT cursor FROM build_sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(db)
            .await?
            .flatten();

    let mut history = History::new(model_name);
    for (n, author, json, skipped, created_at) in rows {
        match serde_json::from_str::<Op>(&json) {
            Ok(op) => history.ops.push(AppliedOp {
                op,
                author,
                at: parse_created_at(&created_at),
                skipped: skipped != 0,
            }),
            Err(e) => {
                tracing::warn!("build: revision {n} of session {session_id} is unreadable: {e}");
            }
        }
    }

    // A stored cursor can outlive the log it pointed into — `truncate_from`
    // shortens the log without touching it — so clamp rather than trust it.
    history.cursor = stored_cursor
        .and_then(|c| usize::try_from(c).ok())
        .unwrap_or(history.ops.len())
        .min(history.ops.len());
    Ok(history)
}

/// Records how many revisions are currently in effect, so undo and redo
/// survive a restart.
///
/// This is session state, not a property of any revision. Marking the tail
/// `skipped` instead — which is what this used to do — cannot express redo:
/// moving the cursor *forward* has to bring revisions back, and a flag the
/// writer only ever sets can never unset itself. Keeping the two apart also
/// leaves `skipped` meaning exactly one thing, an out-of-order
/// [`mark_skipped`].
pub async fn set_cursor<'e, E>(db: E, session_id: &str, cursor: usize) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query("UPDATE build_sessions SET cursor = ? WHERE id = ?")
        .bind(i64::try_from(cursor).unwrap_or(i64::MAX))
        .bind(session_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Parses a `created_at` as SQLite writes it.
///
/// `CURRENT_TIMESTAMP` produces `YYYY-MM-DD HH:MM:SS` in UTC, which is not
/// RFC 3339; rows written by hand may use either. Falls back to the current
/// time so an unreadable timestamp cannot fail a whole session load.
fn parse_created_at(raw: &str) -> chrono::DateTime<chrono::Utc> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
        return parsed.with_timezone(&chrono::Utc);
    }
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
        .map_or_else(|_| chrono::Utc::now(), |naive| naive.and_utc())
}

/// Records the sentence waiting on a part choice, or clears it.
///
/// Cleared on every message that resolves without a pending lookup, so a
/// dropdown left unanswered days ago cannot replay a sentence into a model
/// that has moved on since.
pub async fn set_pending_sentence<'e, E>(
    db: E,
    session_id: &str,
    sentence: Option<&str>,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query("UPDATE build_sessions SET pending_sentence = ? WHERE id = ?")
        .bind(sentence)
        .bind(session_id)
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
        create_build_session(db, "guild1", "thread1", "alice", "Castle", 24, None)
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

        assert!(
            get_active_session_by_channel(&db, "other")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn only_one_active_session_per_channel() {
        let db = setup_db().await;
        session(&db).await;

        // The partial unique index rejects a second live session, so two
        // people running /build at once cannot both win.
        let second = create_build_session(&db, "guild1", "thread1", "bob", "Other", 24, None).await;
        assert!(second.is_err());
    }

    #[tokio::test]
    async fn closing_a_session_frees_the_channel() {
        let db = setup_db().await;
        let first = session(&db).await;
        close_session(&db, &first.id).await.unwrap();

        assert!(
            get_active_session_by_channel(&db, "thread1")
                .await
                .unwrap()
                .is_none()
        );
        // A new session can now be opened in the same thread.
        assert!(
            create_build_session(&db, "guild1", "thread1", "bob", "Second", 24, None)
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
    async fn a_sessions_size_override_round_trips() {
        let db = setup_db().await;
        let given = MaxModelSize { x: 8, y: 6, z: 12 };
        let created =
            create_build_session(&db, "guild1", "thread1", "alice", "Castle", 24, Some(given))
                .await
                .unwrap();
        assert_eq!(created.max_size(), Some(given));

        let reloaded = get_active_session_by_channel(&db, "thread1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.max_size(), Some(given));
    }

    #[tokio::test]
    async fn a_session_opened_without_a_size_has_no_override() {
        let db = setup_db().await;
        let created = session(&db).await;
        assert_eq!(created.max_size(), None);

        let reloaded = get_active_session_by_channel(&db, "thread1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.max_size(), None);
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

    /// Appends `count` operations, each a brick 100 LDU further east.
    async fn append_bricks(db: &SqlitePool, session_id: &str, count: u32) {
        for n in 0..count {
            let x = i32::try_from(n).unwrap() * 100;
            append_revision(db, session_id, n, "alice", &add_op(4, IVec3::new(x, 0, 0)))
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn the_cursor_survives_a_reload_so_undo_is_not_repeated() {
        // Regression: the cursor used to be stored by marking the tail
        // `skipped`, while `load_history` always reset the cursor to the full
        // length. A restart therefore left an undone revision skipped *and*
        // the cursor at the end, so the next `Undo.` removed two operations.
        let db = setup_db().await;
        let s = session(&db).await;
        append_bricks(&db, &s.id, 3).await;

        set_cursor(&db, &s.id, 2).await.unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.revision(), 2, "the cursor is restored, not reset");
        assert_eq!(history.materialize().len(), 2);
        assert_eq!(history.ops.len(), 3, "the redo tail is still on disk");
    }

    #[tokio::test]
    async fn redo_brings_a_revision_back() {
        // Regression: `set_cursor` only ever *added* skips, because its own
        // WHERE clause excluded every row below the new cursor. Moving the
        // cursor forward could therefore never restore anything.
        let db = setup_db().await;
        let s = session(&db).await;
        append_bricks(&db, &s.id, 3).await;

        set_cursor(&db, &s.id, 2).await.unwrap();
        assert_eq!(
            load_history(&db, &s.id, &s.model_name)
                .await
                .unwrap()
                .materialize()
                .len(),
            2
        );

        set_cursor(&db, &s.id, 3).await.unwrap();
        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.revision(), 3);
        assert_eq!(history.materialize().len(), 3, "redo restores the brick");
    }

    #[tokio::test]
    async fn reverting_forward_is_allowed() {
        let db = setup_db().await;
        let s = session(&db).await;
        append_bricks(&db, &s.id, 4).await;

        set_cursor(&db, &s.id, 1).await.unwrap();
        set_cursor(&db, &s.id, 4).await.unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.materialize().len(), 4);
    }

    #[tokio::test]
    async fn an_out_of_order_undo_is_independent_of_the_cursor() {
        // `skipped` and the cursor are different mechanisms and must not
        // interfere: moving the cursor about leaves an out-of-order undo alone.
        let db = setup_db().await;
        let s = session(&db).await;
        append_bricks(&db, &s.id, 3).await;

        mark_skipped(&db, &s.id, 1, true).await.unwrap();
        set_cursor(&db, &s.id, 2).await.unwrap();
        set_cursor(&db, &s.id, 3).await.unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.revision(), 3);
        assert!(history.ops[1].skipped, "the skipped revision stays skipped");
        assert_eq!(history.materialize().len(), 2);
    }

    #[tokio::test]
    async fn a_cursor_beyond_the_log_is_clamped() {
        // A truncated log must not leave a cursor pointing past the end.
        let db = setup_db().await;
        let s = session(&db).await;
        append_bricks(&db, &s.id, 3).await;
        set_cursor(&db, &s.id, 3).await.unwrap();

        truncate_from(&db, &s.id, 1).await.unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(history.revision(), 1);
        assert_eq!(history.materialize().len(), 1);
    }

    #[tokio::test]
    async fn revision_timestamps_are_read_back() {
        // Regression: SQLite's CURRENT_TIMESTAMP is "YYYY-MM-DD HH:MM:SS",
        // which is not RFC 3339, so every `at` silently became "now".
        let db = setup_db().await;
        let s = session(&db).await;
        sqlx::query(
            "INSERT INTO build_revisions (session_id, n, author_user_id, command_json, created_at)
             VALUES (?, 0, 'alice', ?, '2020-01-02 03:04:05')",
        )
        .bind(&s.id)
        .bind(serde_json::to_string(&add_op(4, IVec3::ZERO)).unwrap())
        .execute(&db)
        .await
        .unwrap();

        let history = load_history(&db, &s.id, &s.model_name).await.unwrap();
        assert_eq!(
            history.ops[0].at.to_rfc3339(),
            "2020-01-02T03:04:05+00:00",
            "the stored timestamp is used, not the clock"
        );
    }

    #[tokio::test]
    async fn a_pending_sentence_round_trips_and_can_be_cleared() {
        let db = setup_db().await;
        let s = session(&db).await;
        assert!(s.pending_sentence.is_none());

        set_pending_sentence(&db, &s.id, Some("Add a red 3x12 brick at origin"))
            .await
            .unwrap();
        let found = get_active_session_by_channel(&db, "thread1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            found.pending_sentence.as_deref(),
            Some("Add a red 3x12 brick at origin")
        );

        set_pending_sentence(&db, &s.id, None).await.unwrap();
        assert!(
            get_active_session_by_channel(&db, "thread1")
                .await
                .unwrap()
                .unwrap()
                .pending_sentence
                .is_none()
        );
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
        assert!(
            get_active_session_by_channel(&db, "thread2")
                .await
                .unwrap()
                .is_none()
        );
        // The live session and its revisions are untouched.
        assert!(
            get_active_session_by_channel(&db, "thread1")
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            load_history(&db, &live.id, "Castle")
                .await
                .unwrap()
                .ops
                .len(),
            1
        );
        assert!(
            load_history(&db, "old", "Old")
                .await
                .unwrap()
                .ops
                .is_empty()
        );
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

        assert!(
            get_active_session_by_channel(&db, "thread3")
                .await
                .unwrap()
                .is_none()
        );
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
        assert!(
            get_active_session_by_channel(&db, "thread4")
                .await
                .unwrap()
                .is_none()
        );

        touch_session(&db, "soon", 24).await.unwrap();
        assert!(
            get_active_session_by_channel(&db, "thread4")
                .await
                .unwrap()
                .is_some()
        );
    }
}
