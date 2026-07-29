use crate::config::PocketBaseConfig;
use chrono::{DateTime, Utc};
use futures_util::stream::StreamExt;
use reqwest_eventsource::{Event as SseEvent, EventSource};
use serenity::all::{GuildId, Http};
use sqlx::SqlitePool;
use tokio::time::{Duration, sleep};

pub mod discord;
pub mod pocketbase;

pub use discord::*;
pub use pocketbase::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub url: Option<String>,
    pub tags: Option<Vec<String>>,
    pub image_url: Option<String>,
    pub is_full_day: bool,
    pub updated: String,
}

#[derive(Clone, Debug)]
pub enum SyncMessage {
    SyncStart,
    BatchSync(Vec<Event>),
    SyncEnd,
    RealtimeUpdate {
        action: String,
        record: Box<PbRecord>,
    },
}

pub async fn pocketbase_source_loop(
    http_client: std::sync::Arc<dyn crate::http::HttpProvider>,
    pb_cfg: PocketBaseConfig,
    limit: u64,
    tx: tokio::sync::broadcast::Sender<SyncMessage>,
) {
    loop {
        // 1. Initial full sync
        if let Err(e) = crate::events_sync::pocketbase::stream_pocketbase_events(
            http_client.as_ref(),
            &pb_cfg,
            limit,
            &tx,
        )
        .await
        {
            tracing::error!("Error fetching pocketbase events for full sync: {}", e);
        }

        // 2. Establish SSE Connection
        let url = format!("{}/api/realtime", pb_cfg.url.trim_end_matches('/'));
        let mut es = EventSource::get(&url);

        while let Some(event) = es.next().await {
            match event {
                Ok(SseEvent::Open) => {
                    tracing::info!("PocketBase SSE connection established");
                }
                Ok(SseEvent::Message(msg)) => {
                    if msg.event == "PB_CONNECT" {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&msg.data) {
                            if let Some(client_id) = v.get("clientId").and_then(|s| s.as_str()) {
                                tracing::info!(
                                    "Subscribing to PocketBase Realtime with clientId: {}",
                                    client_id
                                );
                                let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
                                let auth = token.as_ref().map(|t| format!("Bearer {t}"));

                                let payload = serde_json::json!({
                                    "clientId": client_id,
                                    "subscriptions": [format!("{}/*", pb_cfg.collection)]
                                });

                                if let Err(e) = http_client
                                    .post_json_with_auth(&url, auth.as_deref(), &payload, limit)
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to subscribe to PocketBase realtime: {}",
                                        e
                                    );
                                    es.close();
                                }
                            }
                        }
                    } else if msg.event == format!("{}/*", pb_cfg.collection)
                        || msg.event == pb_cfg.collection
                    {
                        if let Ok(rt_msg) = serde_json::from_str::<PbRealtimeMessage>(&msg.data) {
                            let action = if rt_msg.action == "delete"
                                || rt_msg.record.state.as_deref() != Some("published")
                            {
                                "delete"
                            } else {
                                &rt_msg.action
                            };

                            let _ = tx.send(SyncMessage::RealtimeUpdate {
                                action: action.to_string(),
                                record: Box::new(rt_msg.record),
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("PocketBase SSE Error: {}. Reconnecting in 5s...", e);
                    es.close();
                    break;
                }
            }
        }

        sleep(Duration::from_secs(5)).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn discord_sync_worker(
    discord_http: std::sync::Arc<Http>,
    http_client: std::sync::Arc<dyn crate::http::HttpProvider>,
    db: SqlitePool,
    guild_id: GuildId,
    pb_cfg: PocketBaseConfig,
    limit: u64,
    mut rx: tokio::sync::broadcast::Receiver<SyncMessage>,
    sync_mutex: std::sync::Arc<tokio::sync::Mutex<()>>,
) {
    let mut current_uids = std::collections::HashSet::new();
    let mut cached_discord_events = Vec::new();

    while let Ok(msg) = rx.recv().await {
        match msg {
            SyncMessage::SyncStart => {
                current_uids.clear();
                if let Ok(events) = discord_http.get_scheduled_events(guild_id, false).await {
                    cached_discord_events = events;
                }
            }
            SyncMessage::BatchSync(batch) => {
                let _guard = sync_mutex.lock().await;
                let hidden_events = crate::db::get_hidden_events(&db, &guild_id.to_string())
                    .await
                    .unwrap_or_default();
                let hidden_set: std::collections::HashSet<String> =
                    hidden_events.into_iter().collect();

                for pb_ev in batch {
                    if hidden_set.contains(&pb_ev.uid) {
                        continue;
                    }
                    current_uids.insert(pb_ev.uid.clone());
                    let _ = upsert_single_event(
                        &discord_http,
                        http_client.as_ref(),
                        &db,
                        guild_id,
                        &pb_ev,
                        &cached_discord_events,
                        limit,
                    )
                    .await;
                }
            }
            SyncMessage::SyncEnd => {
                let _guard = sync_mutex.lock().await;
                for discord_ev in &cached_discord_events {
                    let desc = discord_ev.description.as_deref().unwrap_or("");
                    let found_uid = desc.find("🆔 ").map(|idx| {
                        let rest = &desc[idx + "🆔 ".len()..];
                        let end = rest.find('\n').unwrap_or(rest.len());
                        rest[..end].trim().to_string()
                    });

                    if let Some(uid) = found_uid {
                        if !current_uids.contains(&uid)
                            && discord_ev.status != serenity::all::ScheduledEventStatus::Completed
                        {
                            let _ = guild_id
                                .delete_scheduled_event(&discord_http, discord_ev.id)
                                .await;
                        }
                    }
                }
                cached_discord_events.clear();
            }
            SyncMessage::RealtimeUpdate { action, record } => {
                let _guard = sync_mutex.lock().await;
                let _ = sync_single_realtime_event(
                    &discord_http,
                    http_client.as_ref(),
                    &db,
                    guild_id,
                    &action,
                    *record,
                    &pb_cfg,
                    limit,
                )
                .await;
            }
        }
    }
}
