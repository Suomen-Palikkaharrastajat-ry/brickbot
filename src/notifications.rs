use serenity::all::{ChannelId, Http, UserId};
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::error;

#[allow(clippy::too_many_arguments)]
pub async fn queue_and_send_notification(
    db: &SqlitePool,
    http: &Arc<Http>,
    input_id: &str,
    kind: &str,
    body: &str,
    user_id: &str,
    channel_id: &str,
    enable_fallback_mention: bool,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    crate::db::insert_notification(db, &id, input_id, kind, body).await?;

    let mut dm_failed = false;
    if let Ok(uid) = user_id.parse::<u64>() {
        let user = UserId::new(uid);
        match user.create_dm_channel(http).await {
            Ok(dm) => match dm.say(http, body).await {
                Ok(msg) => {
                    let _ = crate::db::mark_notification_sent(db, &id, &msg.id.to_string()).await;
                }
                Err(e) => {
                    error!("Failed to send DM: {}", e);
                    dm_failed = true;
                }
            },
            Err(e) => {
                error!("Failed to create DM channel: {}", e);
                dm_failed = true;
            }
        }
    }

    if dm_failed && enable_fallback_mention {
        let fallback_msg = format!(
            "<@{user_id}> You have an update regarding your event submission, but I couldn't send you a DM. Please enable DMs and use the `/events status` command to check it."
        );
        if let Ok(cid) = channel_id.parse::<u64>() {
            if ChannelId::new(cid).say(http, fallback_msg).await.is_ok() {
                let _ = crate::db::mark_notification_dm_failed(db, &id).await;
            }
        }
    }

    Ok(())
}
