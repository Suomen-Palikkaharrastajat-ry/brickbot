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

        let results = search_feed_items(&pool, "42083", "Bugatti Chiron", "Technic")
            .await
            .unwrap();
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

    #[tokio::test]
    async fn test_inputs_extended() {
        let pool = setup_db().await;

        insert_drafting_input(&pool, "drafting1", "{}")
            .await
            .unwrap();
        let payload = get_drafting_payload(&pool, "drafting1").await.unwrap();
        assert_eq!(payload.unwrap(), "{}");

        insert_input_submission(
            &pool, "pending1", "user1", "chan1", "stream1", "topic1", "{}",
        )
        .await
        .unwrap();

        let pending = get_pending_input_by_zulip_topic(&pool, "topic1", Some("stream1"))
            .await
            .unwrap();
        assert!(pending.is_some());
        assert_eq!(pending.as_ref().unwrap().0, "pending1");

        let pending_no_stream = get_pending_input_by_zulip_topic(&pool, "topic1", None)
            .await
            .unwrap();
        assert!(pending_no_stream.is_some());

        update_input_zulip_topic(&pool, "pending1", "topic2")
            .await
            .unwrap();
        update_input_payload(&pool, "pending1", "{\"updated\":true}")
            .await
            .unwrap();

        approve_input_submission(
            &pool,
            "pending1",
            "{\"approved\":true}",
            "mod@example.com",
            "msg1",
        )
        .await
        .unwrap();
        let action = get_input_moderation_action(&pool, "pending1")
            .await
            .unwrap();
        assert_eq!(action.unwrap(), "approve");

        insert_input_submission(
            &pool, "pending2", "user2", "chan2", "stream2", "topic2", "{}",
        )
        .await
        .unwrap();
        reject_input_submission(&pool, "pending2", "mod@example.com", "msg2")
            .await
            .unwrap();
        let action2 = get_input_moderation_action(&pool, "pending2")
            .await
            .unwrap();
        assert_eq!(action2.unwrap(), "reject");

        insert_input_submission(
            &pool, "pending3", "user3", "chan3", "stream3", "topic3", "{}",
        )
        .await
        .unwrap();
        mark_input_answered_by_zulip_topic(&pool, "topic3")
            .await
            .unwrap();

        update_input_status(&pool, "pending1", "custom_status")
            .await
            .unwrap();
        delete_input(&pool, "pending1").await.unwrap();

        insert_event_submission_transaction(
            &pool, "tx1", "user4", "chan4", "stream4", "topic4", "{}", "outbox1", "body1",
        )
        .await
        .unwrap();
        let latest = get_latest_active_input_topic_for_user(&pool, "user4")
            .await
            .unwrap();
        assert!(latest.is_some());

        let discord_msg_topic = get_input_topic_by_discord_message_id(&pool, "some_id")
            .await
            .unwrap();
        assert!(discord_msg_topic.is_none());
    }

    #[tokio::test]
    async fn test_ambient_preferences() {
        let pool = setup_db().await;

        update_user_preferred_services(&pool, "user1", "bricklink,lego")
            .await
            .unwrap();
        let services = get_user_preferred_services(&pool, "user1").await.unwrap();
        assert_eq!(services.unwrap(), "bricklink,lego");

        set_user_ambient_preference(&pool, "user2", true)
            .await
            .unwrap();
        let ambient_ignored = is_user_ambient_ignored(&pool, "user2").await.unwrap();
        assert!(ambient_ignored);

        let training_opt_out = is_user_training_opt_out(&pool, "user1").await.unwrap();
        assert!(!training_opt_out);
    }

    #[tokio::test]
    async fn test_ambient_cooldowns_extended() {
        let pool = setup_db().await;

        set_topic_cooldown(&pool, 123, "LegoSet").await.unwrap();
        let topic_cd = get_topic_cooldown(&pool, 123, "LegoSet").await.unwrap();
        assert!(topic_cd.is_some());

        set_item_cooldown(&pool, 123, "LegoSet", "42083-1")
            .await
            .unwrap();
        let item_cd = get_item_cooldown(&pool, 123, "LegoSet", "42083-1")
            .await
            .unwrap();
        assert!(item_cd.is_some());
    }
}
