use crate::zulip::{AppState, ZulipEventsResponse, ZulipRegisterResponse};
use tracing::{error, info};

#[allow(clippy::too_many_lines)]
pub fn start_event_listener(state: AppState) {
    if state.config.zulip.is_none() {
        return;
    }

    info!("Starting Zulip long-polling event listener");
    tokio::spawn(async move {
        let zulip_cfg = state.config.zulip.as_ref().unwrap();
        let api_key = std::env::var("ZULIP_API_KEY").unwrap_or_default();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let register_url = format!("{}/api/v1/register", zulip_cfg.url.trim_end_matches('/'));

        loop {
            // 1. Register queue
            let register_res = client
                .post(&register_url)
                .basic_auth(&zulip_cfg.bot_email, Some(&api_key))
                .form(&[
                    ("event_types", "[\"message\"]"),
                    ("apply_markdown", "false"),
                ])
                .send()
                .await;

            let mut queue_id;
            let mut last_event_id: i64;

            match register_res {
                Ok(resp) => {
                    let status = resp.status();
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let retry_after = resp
                            .headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|h| h.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(15);

                        error!(
                            "Zulip register API returned status 429 Too Many Requests. Sleeping for {}s.",
                            retry_after
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                        continue;
                    }
                    let text = resp.text().await.unwrap_or_default();
                    match serde_json::from_str::<ZulipRegisterResponse>(&text) {
                        Ok(reg) => {
                            if reg.result == "success" {
                                queue_id = reg.queue_id.unwrap_or_default();
                                last_event_id = reg.last_event_id.unwrap_or(-1);
                                info!("Successfully registered Zulip event queue: {}", queue_id);
                            } else {
                                error!(
                                    "Failed to register Zulip queue: msg={:?} full_response={}",
                                    reg.msg, text
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                                continue;
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to parse Zulip register response: {}. Status: {}, Body: {}",
                                e, status, text
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                            continue;
                        }
                    }
                }
                Err(e) => {
                    error!("Error connecting to Zulip register API: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    continue;
                }
            }

            let sub_url = format!(
                "{}/api/v1/users/me/subscriptions",
                zulip_cfg.url.trim_end_matches('/')
            );
            let sub_payload =
                serde_json::json!([{"name": zulip_cfg.moderation_stream}]).to_string();
            let _ = client
                .post(&sub_url)
                .basic_auth(&zulip_cfg.bot_email, Some(&api_key))
                .form(&[("subscriptions", &sub_payload)])
                .send()
                .await;

            let events_url = format!("{}/api/v1/events", zulip_cfg.url.trim_end_matches('/'));

            let mut last_poll_time = tokio::time::Instant::now();

            // 2. Poll events loop
            loop {
                // Ensure we don't exceed 1 request per second to avoid 429s (Zulip's limit)
                let elapsed = last_poll_time.elapsed();
                if elapsed < std::time::Duration::from_secs(1) {
                    tokio::time::sleep(
                        std::time::Duration::from_secs(1)
                            .checked_sub(elapsed)
                            .unwrap(),
                    )
                    .await;
                }
                last_poll_time = tokio::time::Instant::now();

                let poll_res = client
                    .get(&events_url)
                    .basic_auth(&zulip_cfg.bot_email, Some(&api_key))
                    .query(&[
                        ("queue_id", &queue_id),
                        ("last_event_id", &last_event_id.to_string()),
                    ])
                    .send()
                    .await;

                match poll_res {
                    Ok(resp) => {
                        let status = resp.status();
                        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                            let retry_after = resp
                                .headers()
                                .get(reqwest::header::RETRY_AFTER)
                                .and_then(|h| h.to_str().ok())
                                .and_then(|s| s.parse::<u64>().ok())
                                .unwrap_or(15);

                            error!(
                                "Zulip events API returned status 429 Too Many Requests. Sleeping for {}s.",
                                retry_after
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                            continue;
                        }

                        if !status.is_success() {
                            error!(
                                "Zulip events API returned status {}. Re-registering queue.",
                                status
                            );
                            break; // Break inner loop to re-register
                        }

                        if let Ok(events_resp) = resp.json::<ZulipEventsResponse>().await {
                            if let Some(events) = events_resp.events {
                                for event in events {
                                    last_event_id = last_event_id.max(event.id);
                                    if event.event_type == "message" {
                                        if let Some(msg) = event.message {
                                            // Ensure we don't process our own messages to avoid loops
                                            if msg.sender_email == zulip_cfg.bot_email {
                                                continue;
                                            }

                                            // Process the message
                                            crate::zulip::process_zulip_message(&state, &msg).await;
                                        }
                                    }
                                }
                            } else {
                                error!(
                                    "Zulip events response indicated an error or missing events. Re-registering queue."
                                );
                                break;
                            }
                        } else {
                            error!("Failed to deserialize Zulip events response. Re-registering.");
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Error polling Zulip events: {}. Retrying...", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}
