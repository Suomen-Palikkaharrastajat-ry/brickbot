use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

use crate::config::Config;
use crate::http::HttpProvider;
use crate::zulip::post_topic_to_stream;

#[allow(clippy::too_many_lines)]
pub async fn outbox_worker(pool: SqlitePool, config: Arc<Config>, http: Arc<dyn HttpProvider>) {
    loop {
        if let Err(e) = process_outbox(&pool, &config, &http).await {
            tracing::error!("Outbox worker error: {}", e);
        }
        sleep(Duration::from_secs(5)).await;
    }
}

async fn process_outbox(
    pool: &SqlitePool,
    config: &Arc<Config>,
    http: &Arc<dyn HttpProvider>,
) -> anyhow::Result<()> {
    let rows = crate::db::get_queued_notifications(pool, 10).await?;

    for (id, input_id, _kind, body, attempt_count) in rows {
        if let Some(zulip_cfg) = &config.zulip {
            let stream = &zulip_cfg.moderation_stream;
            let _topic = crate::db::get_input_moderation_action(pool, &input_id).await?;

            let mut parsed_topic = String::new();
            if let Ok(mut body_json) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(t) = body_json.get("topic").and_then(|v| v.as_str()) {
                    parsed_topic = t.to_string();
                }
            }

            if parsed_topic.is_empty() {
                tracing::error!("No topic found in body for outbox id {}", id);
                let _ = crate::db::mark_notification_failed(pool, &id).await;
                continue;
            }

            let content = body; // wait, body is JSON? We can encode topic and content in body json.
            let mut parsed_content = String::new();
            if let Ok(body_json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(c) = body_json.get("content").and_then(|v| v.as_str()) {
                    parsed_content = c.to_string();
                }
            }

            match post_topic_to_stream(
                http.as_ref(),
                zulip_cfg,
                stream,
                &parsed_topic,
                &parsed_content,
                config.resource_limits.max_http_body_bytes,
            )
            .await
            {
                Ok(()) => {
                    let _ =
                        crate::db::update_notification_status(pool, &id, "sent", false, None).await;
                    let _ = crate::db::update_input_status(pool, &input_id, "pending").await;
                }
                Err(e) => {
                    tracing::error!("Failed to send outbox item {}: {}", id, e);
                    let status = if attempt_count >= 5 {
                        "failed"
                    } else {
                        "queued"
                    };
                    let _ = crate::db::update_notification_status(pool, &id, status, true, Some(1))
                        .await;
                }
            }
        }
    }

    Ok(())
}
