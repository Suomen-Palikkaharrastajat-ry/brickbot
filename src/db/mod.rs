use sqlx::SqlitePool;

pub mod ambient;
pub mod events;
pub mod feed;
pub mod inputs;
pub mod notifications;
pub mod workflow_sessions;

pub use ambient::*;
pub use events::*;
pub use feed::*;
pub use inputs::*;
pub use notifications::*;
pub use workflow_sessions::*;

pub(crate) fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    &s[..end]
}

pub async fn cleanup_expired_rows(db: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut total_deleted = 0;

    total_deleted += sqlx::query("DELETE FROM inputs WHERE ttl < CURRENT_TIMESTAMP")
        .execute(db)
        .await?
        .rows_affected();

    total_deleted += sqlx::query("DELETE FROM ambient_cooldowns WHERE ttl < CURRENT_TIMESTAMP")
        .execute(db)
        .await?
        .rows_affected();

    total_deleted +=
        sqlx::query("DELETE FROM ambient_item_cooldowns WHERE ttl < CURRENT_TIMESTAMP")
            .execute(db)
            .await?
            .rows_affected();

    total_deleted += sqlx::query("DELETE FROM ambient_logs WHERE ttl < CURRENT_TIMESTAMP")
        .execute(db)
        .await?
        .rows_affected();

    let _ = sqlx::query("PRAGMA optimize").execute(db).await;

    if total_deleted > 0 {
        tracing::info!(
            "Cleaned up {} expired rows across tables and optimized database",
            total_deleted
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_feed_items() {
        let pool = setup_db().await;

        insert_feed_item(
            &pool,
            "id1",
            "Source 1",
            "Set 42083 is great",
            "This is a cool technic set",
        )
        .await
        .unwrap();

        insert_feed_item(
            &pool,
            "id2",
            "Source 2",
            "Nothing related",
            "Just some text",
        )
        .await
        .unwrap();

        let results = search_feed_items(&pool, "42083").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id1");
        assert_eq!(results[0].source_title, "Source 1");
    }

    #[tokio::test]
    async fn test_ambient_logs() {
        let pool = setup_db().await;
        log_ambient_detection(
            &pool,
            Some("content"),
            "LegoSet",
            0.9,
            "guild1",
            "chan1",
            Some("123"),
            30,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_ambient_cooldown() {
        let pool = setup_db().await;
        defer_ambient_cooldown(&pool, 123, "LegoSet").await.unwrap();
        clear_ambient_cooldown(&pool, 123, "LegoSet").await.unwrap();
    }

    #[tokio::test]
    async fn test_user_ambient_preference() {
        let pool = setup_db().await;
        set_user_ambient_preference(&pool, "user1", true)
            .await
            .unwrap();
        set_user_ambient_preference(&pool, "user1", false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_inputs() {
        let pool = setup_db().await;
        insert_input_submission(&pool, "id1", "user1", "chan1", "stream1", "topic1", "{}")
            .await
            .unwrap();

        insert_draft_submission(&pool, "id2", "user1", "chan1", "{\"draft\":true}")
            .await
            .unwrap();

        let draft = get_draft_submission(&pool, "id2").await.unwrap();
        assert_eq!(draft, "{\"draft\":true}");
    }

    #[tokio::test]
    async fn test_hidden_events() {
        let pool = setup_db().await;

        let hidden = get_hidden_events(&pool, "guild1").await.unwrap();
        assert!(hidden.is_empty());

        hide_event(&pool, "guild1", "brick1").await.unwrap();
        hide_event(&pool, "guild1", "brick2").await.unwrap();
        hide_event(&pool, "guild1", "brick2").await.unwrap();

        let hidden = get_hidden_events(&pool, "guild1").await.unwrap();
        assert_eq!(hidden.len(), 2);
        assert!(hidden.contains(&"brick1".to_string()));
        assert!(hidden.contains(&"brick2".to_string()));

        unhide_event(&pool, "guild1", "brick1").await.unwrap();

        let hidden = get_hidden_events(&pool, "guild1").await.unwrap();
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0], "brick2");
    }
}
