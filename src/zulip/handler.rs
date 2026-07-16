use crate::zulip::{AppState, ZulipMessage, post_topic_to_stream, resolve_zulip_topic};
use serde_json::Value;
use serenity::all::ChannelId;
use tracing::{error, info};

pub fn is_authorized_moderator(config: &crate::config::ZulipConfig, email: &str) -> bool {
    config.moderators.is_empty() || config.moderators.contains(&email.to_string())
}

#[allow(clippy::too_many_lines)]
pub async fn process_zulip_message(state: &AppState, msg: &ZulipMessage) {
    let content = msg.content.trim();
    info!(
        "Received Zulip message in topic '{}': {}",
        msg.subject, content
    );

    let Some(zulip_cfg) = &state.config.zulip else {
        return;
    };

    let limit = state.config.resource_limits.max_http_body_bytes;

    let stream_name = msg.display_recipient.as_str().unwrap_or("");
    let is_moderator = is_authorized_moderator(zulip_cfg, &msg.sender_email);

    let content_lower = content.to_lowercase();
    let mut rest_lower_str = content_lower.clone();
    if content_lower.starts_with("@**")
        || content_lower.starts_with("@bot")
        || content_lower.starts_with('@')
    {
        if let Some((_mention, r)) = content_lower.split_once(' ') {
            rest_lower_str = r.trim().to_string();
        }
    }
    let rest_lower = rest_lower_str.as_str();

    if msg.subject.starts_with("🆕") || msg.subject.starts_with("📝") {
        let pending_input =
            crate::db::get_pending_input_by_zulip_topic(&state.db, &msg.subject, Some(stream_name))
                .await
                .unwrap_or(None);

        info!(
            "Zulip moderation processing. Subject='{}'. pending_input_found={}",
            msg.subject,
            pending_input.is_some()
        );

        if let Some((id, user_id, channel_id, payload_str)) = pending_input {
            let mut notify_discord = false;
            let mut reply_msg_key = None;
            let mut reply_msg_raw = None;

            let mut locale = "en-US".to_string();
            let mut resolved_guild_id = None;
            if let Ok(cid) = channel_id.parse::<u64>() {
                let c = serenity::model::id::ChannelId::new(cid);
                if let Ok(channel) = c.to_channel(&state.discord).await {
                    if let Some(guild_channel) = channel.guild() {
                        let g_id = guild_channel.guild_id.get();
                        resolved_guild_id = Some(g_id);
                        locale = state.config.locale_for(g_id).unwrap_or("en-US").to_string();
                    }
                }
            }

            let cmd_approve =
                rust_i18n::t!("command.zulip.cmd_approve", locale = locale.as_str()).to_lowercase();
            let cmd_approve_published = rust_i18n::t!(
                "command.zulip.cmd_approve_published",
                locale = locale.as_str()
            )
            .to_lowercase();
            let cmd_reject =
                rust_i18n::t!("command.zulip.cmd_reject", locale = locale.as_str()).to_lowercase();

            let trim_chars: &[char] = &[
                ' ', '\t', '\n', '\r', '.', ',', '!', '?', '"', '\'', ':', ';',
            ];
            let clean_rest = rest_lower.trim_matches(trim_chars);
            let clean_cmd_approve = cmd_approve.trim_matches(trim_chars);
            let clean_cmd_approve_published = cmd_approve_published.trim_matches(trim_chars);
            let clean_cmd_reject = cmd_reject.trim_matches(trim_chars);

            info!(
                "Determined locale: '{}', clean_cmd_approve: '{}', clean_rest: '{}'",
                locale, clean_cmd_approve, clean_rest
            );

            let is_approve_published = clean_rest == clean_cmd_approve_published;
            let is_approve = is_approve_published || clean_rest == clean_cmd_approve;
            let is_reject = clean_rest == clean_cmd_reject;
            let is_yaml_update = content.contains("```yaml") || content.contains("```json");

            if (is_approve || is_reject || is_yaml_update) && !is_moderator {
                let err_msg = rust_i18n::t!("command.zulip.unauthorized", locale = locale.as_str());
                let _ = post_topic_to_stream(
                    state.http.as_ref(),
                    zulip_cfg,
                    stream_name,
                    &msg.subject,
                    &err_msg,
                    limit,
                )
                .await;
                return;
            }

            if is_approve {
                info!("Approval received in Zulip topic: {}", msg.subject);
                let mut pb_payload: Value =
                    serde_json::from_str(&payload_str).unwrap_or_else(|_| serde_json::json!({}));

                let is_edit = msg.subject.starts_with("📝");

                if let Some(obj) = pb_payload.as_object_mut() {
                    if is_edit {
                        if let Some(uid_val) = obj.remove("uid") {
                            obj.insert("id".to_string(), uid_val);
                        }
                    } else {
                        obj.remove("uid");
                        obj.remove("id");
                    }
                }

                if !is_edit {
                    if is_approve_published {
                        if let Some(obj) = pb_payload.as_object_mut() {
                            obj.insert("state".to_string(), serde_json::json!("published"));
                        }
                    } else if let Some(obj) = pb_payload.as_object_mut() {
                        obj.insert("state".to_string(), serde_json::json!("draft"));
                    }
                }

                if let Some(pb_cfg) = &state.config.pocketbase {
                    if let Some(obj) = pb_payload.as_object_mut() {
                        let convert_to_utc = |date_str: &str| -> Option<String> {
                            use chrono::TimeZone;
                            let parsed = chrono::NaiveDateTime::parse_from_str(
                                date_str,
                                "%Y-%m-%d %H:%M:%S",
                            )
                            .or_else(|_| {
                                chrono::NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M")
                            });
                            if let Ok(naive) = parsed {
                                if let Some(helsinki) = chrono_tz::Europe::Helsinki
                                    .from_local_datetime(&naive)
                                    .single()
                                {
                                    return Some(
                                        helsinki
                                            .with_timezone(&chrono::Utc)
                                            .format("%Y-%m-%d %H:%M:%S.000Z")
                                            .to_string(),
                                    );
                                }
                            }
                            None
                        };
                        if let Some(start_date) = obj.get("start_date").and_then(|v| v.as_str()) {
                            if let Some(utc_str) = convert_to_utc(start_date) {
                                obj.insert("start_date".to_string(), serde_json::json!(utc_str));
                            }
                        }
                        if let Some(end_date) = obj.get("end_date").and_then(|v| v.as_str()) {
                            if let Some(utc_str) = convert_to_utc(end_date) {
                                obj.insert("end_date".to_string(), serde_json::json!(utc_str));
                            }
                        }
                    }

                    match crate::pocketbase::push_event_data(
                        state.http.as_ref(),
                        pb_cfg,
                        &pb_payload,
                        limit,
                    )
                    .await
                    {
                        Ok(new_pb_id) => {
                            let rows_affected = crate::db::approve_input_submission(
                                &state.db,
                                &id,
                                &serde_json::to_string(&pb_payload).unwrap_or_default(),
                                &msg.sender_email,
                                &msg.id.to_string(),
                            )
                            .await
                            .unwrap_or(0);

                            if rows_affected == 0 {
                                return; // Idempotent or no-op
                            }

                            notify_discord = true;
                            if is_edit {
                                reply_msg_key = Some("command.events.edit_approved");
                            } else {
                                reply_msg_key = Some("command.events.submission_approved");
                            }

                            if let Some(zulip_cfg) = &state.config.zulip {
                                if let Some(stream) = msg.display_recipient.as_str() {
                                    let success_msg = if new_pb_id.is_empty() {
                                        rust_i18n::t!(
                                            "command.zulip.publish_success",
                                            locale = locale.as_str()
                                        )
                                        .to_string()
                                    } else {
                                        let url = format!(
                                            "https://kalenteri.palikkaharrastajat.fi/#/events/{new_pb_id}"
                                        );
                                        rust_i18n::t!(
                                            "command.zulip.publish_success_with_url",
                                            locale = locale.as_str(),
                                            url = url
                                        )
                                        .to_string()
                                    };

                                    let _ = resolve_zulip_topic(
                                        state.http.as_ref(),
                                        zulip_cfg,
                                        &msg.subject,
                                        stream,
                                        &success_msg,
                                        limit,
                                    )
                                    .await;
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to push to PocketBase: {}", e);
                            if let Some(zulip_cfg) = &state.config.zulip {
                                if let Some(stream) = msg.display_recipient.as_str() {
                                    let err_msg = rust_i18n::t!(
                                        "command.zulip.push_failed",
                                        locale = locale.as_str(),
                                        error = e.to_string()
                                    );
                                    let _ = post_topic_to_stream(
                                        state.http.as_ref(),
                                        zulip_cfg,
                                        stream,
                                        &msg.subject,
                                        &err_msg,
                                        limit,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
            } else if is_reject {
                info!("Rejection received in Zulip topic: {}", msg.subject);
                let rows_affected = crate::db::reject_input_submission(
                    &state.db,
                    &id,
                    &msg.sender_email,
                    &msg.id.to_string(),
                )
                .await
                .unwrap_or(0);

                if rows_affected == 0 {
                    return; // Idempotent or no-op
                }

                notify_discord = true;
                reply_msg_key = Some("command.events.submission_rejected");

                if let Some(zulip_cfg) = &state.config.zulip {
                    if let Some(stream) = msg.display_recipient.as_str() {
                        let _ = resolve_zulip_topic(
                            state.http.as_ref(),
                            zulip_cfg,
                            &msg.subject,
                            stream,
                            &rust_i18n::t!(
                                "command.zulip.topic_resolved",
                                locale = locale.as_str()
                            ),
                            limit,
                        )
                        .await;
                    }
                }
            } else if content.contains("```yaml") || content.contains("```json") {
                let mut yaml_str = content;
                if let Some(start_idx) = yaml_str
                    .find("```yaml")
                    .or_else(|| yaml_str.find("```json"))
                {
                    let block_start = yaml_str[start_idx..]
                        .find('\n')
                        .map_or(start_idx + 7, |i| start_idx + i + 1);
                    let remainder = &yaml_str[block_start..];
                    yaml_str = remainder
                        .find("```")
                        .map_or(remainder, |end_idx| &remainder[..end_idx])
                        .trim();
                }

                if let Ok(parsed) = serde_yaml::from_str::<serde_json::Value>(yaml_str) {
                    let mut existing_payload: Value = serde_json::from_str(&payload_str)
                        .unwrap_or_else(|_| serde_json::json!({}));

                    if let (Some(existing_obj), Some(new_obj)) =
                        (existing_payload.as_object_mut(), parsed.as_object())
                    {
                        for (k, v) in new_obj {
                            if k == "type" {
                                existing_obj.insert("tags".to_string(), serde_json::json!([v]));
                                existing_obj.remove("type");
                            } else {
                                existing_obj.insert(k.clone(), v.clone());
                            }
                        }
                    } else {
                        existing_payload = parsed;
                        if let Some(obj) = existing_payload.as_object_mut() {
                            if let Some(t) = obj.remove("type") {
                                obj.insert("tags".to_string(), serde_json::json!([t]));
                            }
                        }
                    }

                    let _ = crate::db::update_input_payload(
                        &state.db,
                        &id,
                        &existing_payload.to_string(),
                    )
                    .await;

                    let mut current_subject = msg.subject.clone();

                    if let Some(zulip_cfg) = &state.config.zulip {
                        let p_title = existing_payload
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let p_start_date = existing_payload
                            .get("start_date")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let p_uid = existing_payload
                            .get("uid")
                            .or_else(|| existing_payload.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        let topic_prefix = if msg.subject.starts_with("🆕") {
                            "🆕"
                        } else {
                            "📝"
                        };
                        let expected_topic = crate::workflows::events::format_zulip_topic(
                            topic_prefix,
                            p_start_date,
                            p_title,
                            p_uid,
                        );

                        if expected_topic != msg.subject {
                            let api_key = std::env::var("ZULIP_API_KEY").unwrap_or_default();
                            let patch_url = format!(
                                "{}/api/v1/messages/{}",
                                zulip_cfg.url.trim_end_matches('/'),
                                msg.id
                            );
                            let patch_form = vec![
                                ("topic".to_string(), expected_topic.clone()),
                                ("propagate_mode".to_string(), "change_all".to_string()),
                            ];
                            let _ = state
                                .http
                                .patch_form_basic_auth(
                                    &patch_url,
                                    &zulip_cfg.bot_email,
                                    Some(&api_key),
                                    patch_form,
                                    limit,
                                )
                                .await;

                            let _ = crate::db::update_input_zulip_topic(
                                &state.db,
                                &id,
                                &expected_topic,
                            )
                            .await;

                            current_subject = expected_topic;
                        }

                        if let Some(stream) = msg.display_recipient.as_str() {
                            let msg_updated = rust_i18n::t!(
                                "command.zulip.payload_updated",
                                locale = locale.as_str()
                            );
                            let _ = post_topic_to_stream(
                                state.http.as_ref(),
                                zulip_cfg,
                                stream,
                                &current_subject,
                                &msg_updated,
                                limit,
                            )
                            .await;

                            let api_key = std::env::var("ZULIP_API_KEY").unwrap_or_default();
                            let narrow = serde_json::json!([
                                {"operator": "topic", "operand": current_subject}
                            ])
                            .to_string();
                            if let Ok(url) = reqwest::Url::parse_with_params(
                                &format!("{}/api/v1/messages", zulip_cfg.url.trim_end_matches('/')),
                                &[
                                    ("narrow", &narrow),
                                    ("anchor", &"oldest".to_string()),
                                    ("num_before", &"0".to_string()),
                                    ("num_after", &"1".to_string()),
                                ],
                            ) {
                                let messages_url = url.to_string();
                                if let Ok(messages_resp) = state
                                    .http
                                    .get_text_basic_auth(
                                        &messages_url,
                                        &zulip_cfg.bot_email,
                                        Some(&api_key),
                                        limit,
                                    )
                                    .await
                                {
                                    if let Ok(json) =
                                        serde_json::from_str::<serde_json::Value>(&messages_resp)
                                    {
                                        if let Some(messages) =
                                            json.get("messages").and_then(|m| m.as_array())
                                        {
                                            if let Some(first_msg) = messages.first() {
                                                if let Some(first_msg_id) = first_msg
                                                    .get("id")
                                                    .and_then(serde_json::Value::as_u64)
                                                {
                                                    let p_title = existing_payload
                                                        .get("title")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    let p_start_date = existing_payload
                                                        .get("start_date")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    let p_end_date = existing_payload
                                                        .get("end_date")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    let p_location = existing_payload
                                                        .get("location")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    let p_description = existing_payload
                                                        .get("description")
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    let p_uid = existing_payload
                                                        .get("uid")
                                                        .or_else(|| existing_payload.get("id"))
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    let p_image_url = existing_payload
                                                        .get("image_url")
                                                        .and_then(|v| v.as_str());

                                                    let display_payload =
                                                        crate::workflows::events::modals::generate_display_yaml(&existing_payload);

                                                    let diff_payload = crate::workflows::events::modals::generate_diff_str(
                                                        state.config.pocketbase.as_ref(),
                                                        state.http.as_ref(),
                                                        state.config.resource_limits.max_http_body_bytes,
                                                        p_uid,
                                                        &display_payload
                                                    ).await;

                                                    let cmd_approve = rust_i18n::t!(
                                                        "command.zulip.cmd_approve",
                                                        locale = locale.as_str()
                                                    );
                                                    let cmd_approve_published = rust_i18n::t!(
                                                        "command.zulip.cmd_approve_published",
                                                        locale = locale.as_str()
                                                    );
                                                    let cmd_reject = rust_i18n::t!(
                                                        "command.zulip.cmd_reject",
                                                        locale = locale.as_str()
                                                    );

                                                    let new_content = if current_subject
                                                        .starts_with("🆕")
                                                    {
                                                        p_image_url.map_or_else(
                                                            || {
                                                                rust_i18n::t!(
                                                                    "command.zulip.new_event_no_image",
                                                                    locale = locale.as_str(),
                                                                    title = p_title,
                                                                    start_date = p_start_date,
                                                                    end_date = p_end_date,
                                                                    location = p_location,
                                                                    description = p_description,
                                                                    payload_str = diff_payload,
                                                                    cmd_approve = cmd_approve,
                                                                    cmd_approve_published = cmd_approve_published,
                                                                    cmd_reject = cmd_reject
                                                                ).to_string()
                                                            },
                                                            |url| {
                                                                rust_i18n::t!(
                                                                    "command.zulip.new_event",
                                                                    locale = locale.as_str(),
                                                                    title = p_title,
                                                                    start_date = p_start_date,
                                                                    end_date = p_end_date,
                                                                    location = p_location,
                                                                    description = p_description,
                                                                    url = url,
                                                                    payload_str = diff_payload,
                                                                    cmd_approve = cmd_approve,
                                                                    cmd_approve_published = cmd_approve_published,
                                                                    cmd_reject = cmd_reject
                                                                ).to_string()
                                                            }
                                                        )
                                                    } else {
                                                        rust_i18n::t!(
                                                            "command.zulip.edit_event",
                                                            locale = locale.as_str(),
                                                            uid = p_uid,
                                                            title = p_title,
                                                            start_date = p_start_date,
                                                            end_date = p_end_date,
                                                            location = p_location,
                                                            description = p_description,
                                                            payload_str = diff_payload,
                                                            yaml_str = display_payload,
                                                            cmd_approve = cmd_approve,
                                                            cmd_reject = cmd_reject
                                                        )
                                                        .to_string()
                                                    };

                                                    let patch_msg_url = format!(
                                                        "{}/api/v1/messages/{}",
                                                        zulip_cfg.url.trim_end_matches('/'),
                                                        first_msg_id
                                                    );
                                                    let patch_msg_form =
                                                        vec![("content".to_string(), new_content)];
                                                    let _ = state
                                                        .http
                                                        .patch_form_basic_auth(
                                                            &patch_msg_url,
                                                            &zulip_cfg.bot_email,
                                                            Some(&api_key),
                                                            patch_msg_form,
                                                            limit,
                                                        )
                                                        .await;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(zulip_cfg) = &state.config.zulip {
                    if let Some(stream) = msg.display_recipient.as_str() {
                        let msg_err = rust_i18n::t!(
                            "command.zulip.payload_parse_failed",
                            locale = locale.as_str()
                        );
                        let _ = post_topic_to_stream(
                            state.http.as_ref(),
                            zulip_cfg,
                            stream,
                            &msg.subject,
                            &msg_err,
                            limit,
                        )
                        .await;
                    }
                }
            } else {
                if !is_moderator {
                    let err_msg =
                        rust_i18n::t!("command.zulip.unauthorized", locale = locale.as_str());
                    let _ = post_topic_to_stream(
                        state.http.as_ref(),
                        zulip_cfg,
                        stream_name,
                        &msg.subject,
                        &err_msg,
                        limit,
                    )
                    .await;
                    return;
                }
                let mut actual_msg = content;
                if content_lower.starts_with("@**")
                    || content_lower.starts_with("@bot")
                    || content_lower.starts_with('@')
                {
                    if let Some((_, r)) = content.split_once(' ') {
                        actual_msg = r.trim();
                    }
                }
                if actual_msg.to_lowercase().starts_with("reply") {
                    actual_msg = actual_msg[5..].trim();
                }

                // Ignore Zulip system messages about topic moves
                if actual_msg.contains("Tämä aihe on siirretty paikasta")
                    || actual_msg.contains("This topic was moved from")
                {
                    actual_msg = "";
                }
                if !actual_msg.is_empty() {
                    notify_discord = true;

                    let mut event_title = String::new();
                    if let Ok(payload_json) =
                        serde_json::from_str::<serde_json::Value>(&payload_str)
                    {
                        if let Some(t) = payload_json.get("title").and_then(|v| v.as_str()) {
                            event_title = t.to_string();
                        }
                    }

                    let msg_moderator = rust_i18n::t!(
                        "command.zulip.moderator_asked",
                        locale = locale.as_str(),
                        title = event_title,
                        msg = actual_msg
                    );
                    reply_msg_raw = Some(msg_moderator.to_string());
                }
            }

            let mut event_title = String::new();
            if let Ok(payload_json) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                if let Some(t) = payload_json.get("title").and_then(|v| v.as_str()) {
                    event_title = t.to_string();
                } else if let Some(t) = payload_json.get("summary").and_then(|v| v.as_str()) {
                    event_title = t.to_string();
                }
            }

            if notify_discord {
                let reply_msg = reply_msg_key.map_or_else(
                    || reply_msg_raw.unwrap_or_default(),
                    |key| {
                        rust_i18n::t!(key, locale = locale.as_str(), title = event_title.clone())
                            .to_string()
                    },
                );

                if !reply_msg.is_empty() {
                    let kind = if reply_msg_key == Some("command.events.submission_approved") {
                        "approved"
                    } else if reply_msg_key == Some("command.events.submission_rejected") {
                        "rejected"
                    } else {
                        "question"
                    };

                    let _ = crate::notifications::queue_and_send_notification(
                        &state.db,
                        &state.discord,
                        &id,
                        kind,
                        &reply_msg,
                        &user_id,
                        &channel_id,
                        resolved_guild_id.map_or(
                            state.config.commands.events.enable_fallback_mention,
                            |g_id| {
                                state
                                    .config
                                    .commands_for(g_id)
                                    .events
                                    .enable_fallback_mention
                            },
                        ),
                    )
                    .await;
                }
            }
        }
    } else {
        // Treat as a reply back to Discord for non-event topics
        let locale = state
            .config
            .locale
            .clone()
            .unwrap_or_else(|| "fi-FI".to_string());
        let mut rest = content;
        if content_lower.starts_with("@**")
            || content_lower.starts_with("@bot")
            || content_lower.starts_with('@')
        {
            if let Some((_mention, r)) = content.split_once(' ') {
                rest = r.trim();
            }
        }
        if rest.to_lowercase().starts_with("reply") {
            rest = rest[5..].trim();
        }

        if !rest.is_empty() {
            if let Ok(channel_id) = msg.subject.parse::<u64>() {
                if let Err(e) = ChannelId::new(channel_id).say(&state.discord, rest).await {
                    error!("Failed to forward Zulip reply to Discord: {}", e);
                } else {
                    info!("Forwarded reply to Discord channel {}", channel_id);
                    if let Some(zulip_cfg) = &state.config.zulip {
                        if let Some(stream) = msg.display_recipient.as_str() {
                            let _ = resolve_zulip_topic(
                                state.http.as_ref(),
                                zulip_cfg,
                                &msg.subject,
                                stream,
                                &rust_i18n::t!(
                                    "command.zulip.topic_resolved",
                                    locale = locale.as_str()
                                ),
                                limit,
                            )
                            .await;
                        }
                    }
                }
            } else if msg.subject.starts_with("Question-") {
                let pending_input =
                    crate::db::get_pending_input_by_zulip_topic(&state.db, &msg.subject, None)
                        .await
                        .unwrap_or(None);

                if let Some((id, user_id, channel_id, _payload_str)) = pending_input {
                    let mut locale = "en-US".to_string();
                    let mut resolved_guild_id = None;
                    if let Ok(cid) = channel_id.parse::<u64>() {
                        let c = serenity::model::id::ChannelId::new(cid);
                        if let Ok(channel) = c.to_channel(&state.discord).await {
                            if let Some(guild_channel) = channel.guild() {
                                let g_id = guild_channel.guild_id.get();
                                resolved_guild_id = Some(g_id);
                                locale =
                                    state.config.locale_for(g_id).unwrap_or("en-US").to_string();
                            }
                        }
                    }

                    let translated_reply = rust_i18n::t!(
                        "command.zulip.support_replied",
                        locale = locale.as_str(),
                        msg = rest
                    );

                    let _ = crate::notifications::queue_and_send_notification(
                        &state.db,
                        &state.discord,
                        &id,
                        "question",
                        &translated_reply,
                        &user_id,
                        &channel_id,
                        resolved_guild_id.map_or(
                            state.config.commands.events.enable_fallback_mention,
                            |g_id| {
                                state
                                    .config
                                    .commands_for(g_id)
                                    .events
                                    .enable_fallback_mention
                            },
                        ),
                    )
                    .await;

                    let _ = crate::db::mark_input_answered_by_zulip_topic(&state.db, &msg.subject)
                        .await;
                    if let Some(zulip_cfg) = &state.config.zulip {
                        if let Some(stream) = msg.display_recipient.as_str() {
                            let _ = resolve_zulip_topic(
                                state.http.as_ref(),
                                zulip_cfg,
                                &msg.subject,
                                stream,
                                &rust_i18n::t!(
                                    "command.zulip.topic_resolved",
                                    locale = locale.as_str()
                                ),
                                limit,
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ZulipConfig;

    #[test]
    fn test_is_authorized_moderator() {
        let config_with_mods = ZulipConfig {
            url: "https://zulip.example.com".to_string(),
            bot_email: "bot@example.com".to_string(),
            moderation_stream: "mod".to_string(),
            support_stream: Some("support".to_string()),
            moderators: vec!["mod@example.com".to_string()],
        };

        // Standard moderation check
        assert!(is_authorized_moderator(
            &config_with_mods,
            "mod@example.com"
        ));
        assert!(!is_authorized_moderator(
            &config_with_mods,
            "user@example.com"
        ));

        let config_empty_mods = ZulipConfig {
            url: "https://zulip.example.com".to_string(),
            bot_email: "bot@example.com".to_string(),
            moderation_stream: "mod".to_string(),
            support_stream: Some("support".to_string()),
            moderators: vec![],
        };

        // Red TDD: Should allow any user if list is empty
        assert!(is_authorized_moderator(
            &config_empty_mods,
            "anyone@example.com"
        ));
    }
}
