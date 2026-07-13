use crate::workflows::events::{
    InteractionVisibility, WizardInteraction, format_zulip_topic, validate_event_dates,
};
use crate::workflows::{AppContext, extract_locale_and_bot_name};
use chrono::{TimeZone, Timelike};
use chrono_tz::Europe::Helsinki;
use serenity::all::{
    ActionRowComponent, ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, Context,
    CreateActionRow, CreateButton, CreateInputText, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateModal, InputTextStyle, ModalInteraction,
};

#[derive(serde::Serialize)]
struct EventDisplayPayload<'a> {
    title: &'a str,
    start_date: &'a str,
    end_date: &'a str,
    description: String,
    location: &'a str,
    #[serde(rename = "type")]
    event_type: &'a str,
    url: &'a str,
}

fn generate_display_yaml(payload: &serde_json::Value) -> String {
    let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let start_date = payload
        .get("start_date")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let end_date = payload
        .get("end_date")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut description = payload
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if !description.ends_with('\n') {
        description.push('\n');
    }
    let location = payload
        .get("location")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let event_type = payload
        .get("tags")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("event");
    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("");

    let display_obj = EventDisplayPayload {
        title,
        start_date,
        end_date,
        description,
        location,
        event_type,
        url,
    };
    serde_yaml::to_string(&display_obj).unwrap_or_default()
}

pub async fn handle_wizard_retry_submit(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id_arg: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(sid) = session_id_arg {
        let user_id = interaction.user.id.get().to_string();
        let guild_id = interaction
            .guild_id
            .map(|g| g.get().to_string())
            .unwrap_or_default();
        let channel_id = interaction.channel_id.get().to_string();

        let payload_opt = crate::db::authorize_and_consume_session(
            &app_ctx.db,
            sid,
            "retry_submit",
            &user_id,
            &guild_id,
        )
        .await?;

        let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

        if let Some(p) = payload_opt {
            if let Some(draft_id) = p.get("draft_id").and_then(|v| v.as_str()) {
                if let Ok(payload_str) =
                    crate::db::get_draft_submission(&app_ctx.db, draft_id).await
                {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_str) {
                        let is_edit = payload
                            .get("is_edit")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        let uid = payload.get("uid").and_then(|v| v.as_str()).unwrap_or("");
                        let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("");
                        let dates = payload.get("dates").and_then(|v| v.as_str()).map_or_else(
                            || {
                                let start_date = payload
                                    .get("start_date")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let end_date = payload
                                    .get("end_date")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if end_date.is_empty() {
                                    start_date.to_owned()
                                } else {
                                    format!("{start_date} - {end_date}")
                                }
                            },
                            str::to_owned,
                        );
                        let location = payload
                            .get("location")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        let tag = payload
                            .get("tag")
                            .and_then(|v| v.as_str())
                            .unwrap_or("event");
                        let description = payload
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        let session_payload = serde_json::json!({
                            "draft_id": if is_edit { None } else { Some(draft_id) },
                            "uid": if is_edit { Some(uid) } else { None },
                            "tag": tag
                        });

                        let session_id = crate::db::create_workflow_session(
                            &app_ctx.db,
                            if is_edit {
                                "edit_event"
                            } else {
                                "submit_event"
                            },
                            &user_id,
                            &guild_id,
                            &channel_id,
                            session_payload,
                            15,
                        )
                        .await?;

                        let modal_id = if is_edit {
                            format!("modal_edit_event_{session_id}")
                        } else {
                            format!("modal_submit_event_{session_id}")
                        };

                        let modal = CreateModal::new(
                            modal_id,
                            if is_edit {
                                "Edit Event"
                            } else {
                                "Submit an Event"
                            },
                        )
                        .components(vec![
                            CreateActionRow::InputText(
                                CreateInputText::new(InputTextStyle::Short, "Title", "title")
                                    .required(true)
                                    .value(title),
                            ),
                            CreateActionRow::InputText(
                                CreateInputText::new(
                                    InputTextStyle::Short,
                                    "Dates (YYYY-MM-DD HH:MM - HH:MM)",
                                    "dates",
                                )
                                .required(true)
                                .value(dates),
                            ),
                            CreateActionRow::InputText(
                                CreateInputText::new(InputTextStyle::Short, "Location", "location")
                                    .required(false)
                                    .value(location),
                            ),
                            CreateActionRow::InputText(
                                CreateInputText::new(InputTextStyle::Short, "URL", "url")
                                    .required(false)
                                    .value(url),
                            ),
                            CreateActionRow::InputText(
                                CreateInputText::new(
                                    InputTextStyle::Paragraph,
                                    "Description",
                                    "description",
                                )
                                .required(false)
                                .value(description),
                            ),
                        ]);

                        interaction
                            .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
                            .await?;

                        let msg = rust_i18n::t!(
                            "command.events.opening_edit_form",
                            locale = locale.as_str()
                        );
                        let _ = interaction
                            .edit_response(
                                &ctx.http,
                                serenity::builder::EditInteractionResponse::new()
                                    .content(msg.as_ref())
                                    .components(vec![]),
                            )
                            .await;
                    }
                }
            }
        } else {
            let resp = CreateInteractionResponseMessage::new()
                .content(rust_i18n::t!("errors.expired", locale = locale.as_str()))
                .ephemeral(true);
            let _ = interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    }
    Ok(())
}

pub async fn handle_wizard_submit_tag_continue(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id_arg: Option<&str>,
) -> anyhow::Result<()> {
    let mut tag = "event".to_string();
    let mut draft_id_opt: Option<String> = None;

    let user_id = interaction.user.id.get().to_string();
    let guild_id = interaction
        .guild_id
        .map(|g| g.get().to_string())
        .unwrap_or_default();
    let channel_id = interaction.channel_id.get().to_string();

    if let Some(sid) = session_id_arg {
        let payload_opt = crate::db::authorize_and_consume_session(
            &app_ctx.db,
            sid,
            "main_events_wizard",
            &user_id,
            &guild_id,
        )
        .await?;
        if let Some(p) = payload_opt {
            draft_id_opt = p
                .get("draft_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            if let Some(t) = p.get("selected_tag").and_then(|v| v.as_str()) {
                tag = t.to_string();
            }
        }
    }

    let payload = serde_json::json!({
        "draft_id": draft_id_opt,
        "tag": tag
    });

    let session_id = crate::db::create_workflow_session(
        &app_ctx.db,
        "submit_event",
        &user_id,
        &guild_id,
        &channel_id,
        payload,
        15,
    )
    .await?;

    let modal = create_submit_modal(&session_id);
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;

    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;
    let msg = rust_i18n::t!(
        "command.events.opening_submit_form",
        locale = locale.as_str()
    );
    let _ = interaction
        .edit_response(
            &ctx.http,
            serenity::builder::EditInteractionResponse::new()
                .content(msg.as_ref())
                .components(vec![]),
        )
        .await;

    Ok(())
}

pub fn create_submit_modal(session_id: &str) -> CreateModal {
    let custom_id = format!("modal_submit_event_{session_id}");

    let now_utc = chrono::Utc::now();
    let now_helsinki = now_utc.with_timezone(&Helsinki);
    let start_helsinki = now_helsinki
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    let end_helsinki = start_helsinki + chrono::Duration::hours(1);

    let dates_str = format!(
        "{} - {}",
        start_helsinki.format("%Y-%m-%d %H:%M"),
        end_helsinki.format("%Y-%m-%d %H:%M")
    );

    CreateModal::new(custom_id, "Submit an Event").components(vec![
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "Title", "title").required(true),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(
                InputTextStyle::Short,
                "Dates (YYYY-MM-DD HH:MM - HH:MM)",
                "dates",
            )
            .required(true)
            .value(&dates_str),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "Location", "location").required(false),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "URL", "url").required(false),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Paragraph, "Description", "description")
                .required(false),
        ),
    ])
}

fn parse_submit_modal_custom_id(custom_id: &str) -> Option<String> {
    custom_id
        .strip_prefix("modal_submit_event_")
        .map(str::to_owned)
}

pub async fn handle_modal_submit_event(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    if let Some(msg) = interaction.message.as_ref() {
        let _ = msg.delete(&ctx.http).await;
    }

    let custom_id = interaction.data.custom_id.as_str();
    let Some(session_id) = parse_submit_modal_custom_id(custom_id) else {
        return Ok(());
    };

    let user_id = interaction.user.id.get().to_string();
    let guild_id = interaction
        .guild_id
        .map(|g| g.get().to_string())
        .unwrap_or_default();

    let Some(payload) = crate::db::authorize_and_consume_session(
        &app_ctx.db,
        &session_id,
        "submit_event",
        &user_id,
        &guild_id,
    )
    .await?
    else {
        let resp = CreateInteractionResponseMessage::new()
            .content(rust_i18n::t!(
                "errors.session_consumed",
                locale = locale.as_str()
            ))
            .ephemeral(true);
        let _ = interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
        return Ok(());
    };

    let tags = payload
        .get("tag")
        .and_then(|v| v.as_str())
        .unwrap_or("event")
        .to_string();
    let draft_uuid = payload
        .get("draft_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let mut title = String::new();
    let mut dates_str = String::new();
    let mut location = String::new();
    let mut url = String::new();
    let mut description = String::new();

    for row in &interaction.data.components {
        if let Some(ActionRowComponent::InputText(input)) = row.components.first() {
            match input.custom_id.as_str() {
                "title" => title = input.value.clone().unwrap_or_default(),
                "dates" => dates_str = input.value.clone().unwrap_or_default(),
                "location" => location = input.value.clone().unwrap_or_default(),
                "url" => url = input.value.clone().unwrap_or_default(),
                "description" => description = input.value.clone().unwrap_or_default(),
                _ => {}
            }
        }
    }

    let mut validation_errors = Vec::new();

    let title_res = crate::workflows::events::validate_text_field(&title, "Title", 100, true);
    if let Err(e) = &title_res {
        validation_errors.push(e.clone());
    }

    let location_res =
        crate::workflows::events::validate_text_field(&location, "Location", 100, false);
    if let Err(e) = &location_res {
        validation_errors.push(e.clone());
    }

    let url_res = crate::workflows::events::validate_url(&url);
    if let Err(e) = &url_res {
        validation_errors.push(e.clone());
    }

    let description_res =
        crate::workflows::events::validate_text_field(&description, "Description", 2000, false);
    if let Err(e) = &description_res {
        validation_errors.push(e.clone());
    }

    let dates_res = validate_event_dates(&dates_str);
    if let Err(e) = &dates_res {
        validation_errors.push(e.clone());
    }

    if !validation_errors.is_empty() {
        let draft_id = uuid::Uuid::new_v4().to_string();
        let draft_payload = serde_json::json!({
            "title": title,
            "dates": dates_str,
            "location": location,
            "url": url,
            "tag": tags,
            "description": description,
            "is_edit": false,
            "uid": null,
        });

        let _ = crate::db::insert_draft_submission(
            &app_ctx.db,
            &draft_id,
            &interaction.user.id.to_string(),
            &interaction.channel_id.to_string(),
            &draft_payload.to_string(),
        )
        .await;

        let session_payload = serde_json::json!({ "draft_id": draft_id });
        let retry_session_id = crate::db::create_workflow_session(
            &app_ctx.db,
            "retry_submit",
            &interaction.user.id.to_string(),
            &interaction
                .guild_id
                .map(|g| g.get().to_string())
                .unwrap_or_default(),
            &interaction.channel_id.to_string(),
            session_payload,
            15,
        )
        .await?;

        let msg = rust_i18n::t!(
            "errors.validation_failed",
            locale = locale.as_str(),
            e = validation_errors.join("\n")
        );
        let resp = CreateInteractionResponseMessage::new()
            .content(msg)
            .components(vec![CreateActionRow::Buttons(vec![
                CreateButton::new(format!("wizard_retry_submit:{retry_session_id}"))
                    .label(rust_i18n::t!(
                        "command.events.edit_and_retry",
                        locale = locale.as_str()
                    ))
                    .style(ButtonStyle::Primary),
            ])])
            .ephemeral(true);
        let _ = interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
        return Ok(());
    }

    let (pb_start_date, pb_end_date) = dates_res.unwrap();
    let valid_title = title_res.unwrap();
    let valid_location = location_res.unwrap();
    let valid_url = url_res.unwrap();
    let valid_desc = description_res.unwrap();

    let mut image_url: Option<String> = None;
    if let Some(ref uuid) = draft_uuid {
        let payload_opt = crate::db::get_drafting_payload(&app_ctx.db, uuid).await?;
        if let Some(json_str) = payload_opt {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let Some(url) = json.get("image_url").and_then(|v| v.as_str()) {
                    image_url = Some(url.to_string());
                }
            }
            let _ = crate::db::delete_input(&app_ctx.db, uuid).await;
        }
    }

    let generated_uid = uuid::Uuid::new_v4().as_simple().to_string()[..15].to_string();

    let mut tags_array = vec![];
    if !tags.is_empty() {
        tags_array.push(tags);
    }

    let mod_payload = crate::workflows::events::ModerationPayload {
        id: generated_uid.clone(),
        title: valid_title.clone(),
        start_date: pb_start_date.clone(),
        end_date: pb_end_date.clone(),
        location: valid_location.clone(),
        url: valid_url.clone(),
        tags: tags_array,
        description: valid_desc.clone(),
        state: "draft".to_string(),
        image_url: image_url.clone(),
    };

    let payload_str = serde_json::to_string(&mod_payload).unwrap();

    let user_id = interaction.user.id.get().to_string();
    let channel_id = interaction.channel_id.get().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let topic_prefix = rust_i18n::t!("command.zulip.topic_new_event", locale = locale.as_str());
    let start_date = pb_start_date.clone();
    let zulip_topic = format_zulip_topic(&topic_prefix, &start_date, &valid_title, &generated_uid);

    if let Some(zulip_cfg) = &app_ctx.config.zulip {
        let display_payload = generate_display_yaml(&serde_json::to_value(&mod_payload).unwrap());
        let cmd_approve = rust_i18n::t!("command.zulip.cmd_approve", locale = locale.as_str());
        let cmd_approve_published = rust_i18n::t!(
            "command.zulip.cmd_approve_published",
            locale = locale.as_str()
        );
        let cmd_reject = rust_i18n::t!("command.zulip.cmd_reject", locale = locale.as_str());

        let content = image_url.map_or_else(
            || {
                rust_i18n::t!(
                    "command.zulip.new_event_no_image",
                    locale = locale.as_str(),
                    title = valid_title,
                    start_date = pb_start_date,
                    end_date = pb_end_date,
                    location = valid_location,
                    description = valid_desc,
                    payload_str = display_payload,
                    cmd_approve = cmd_approve,
                    cmd_approve_published = cmd_approve_published,
                    cmd_reject = cmd_reject
                )
                .to_string()
            },
            |url| {
                rust_i18n::t!(
                    "command.zulip.new_event",
                    locale = locale.as_str(),
                    title = valid_title,
                    start_date = pb_start_date,
                    end_date = pb_end_date,
                    location = valid_location,
                    description = valid_desc,
                    url = url,
                    payload_str = display_payload,
                    cmd_approve = cmd_approve,
                    cmd_approve_published = cmd_approve_published,
                    cmd_reject = cmd_reject
                )
                .to_string()
            },
        );

        let outbox_id = uuid::Uuid::new_v4().to_string();
        let outbox_body = serde_json::json!({
            "topic": zulip_topic,
            "content": content
        })
        .to_string();

        let _ = crate::db::insert_event_submission_transaction(
            &app_ctx.db,
            &id,
            &user_id,
            &channel_id,
            &zulip_cfg.moderation_stream,
            &zulip_topic,
            &payload_str,
            &outbox_id,
            &outbox_body,
        )
        .await;

        let msg = rust_i18n::t!(
            "command.events.submission_received",
            locale = locale.as_str()
        );
        let final_msg = if msg.is_empty() || msg == "command.events.submission_received" {
            "Your event has been submitted for moderation review."
        } else {
            &msg
        };
        let resp = CreateInteractionResponseMessage::new()
            .content(final_msg)
            .ephemeral(true);
        let _ = interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
    }

    Ok(())
}

pub async fn handle_wizard_edit_event_select_change(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    if let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind {
        if let Some(uid) = values.first() {
            let user_id = interaction.user.id.get().to_string();
            if let Some(mut payload) =
                crate::db::get_workflow_session_payload(&app_ctx.db, session_id, &user_id).await?
            {
                payload["selected_event_uid"] = serde_json::Value::String(uid.clone());
                crate::db::update_workflow_session_payload(
                    &app_ctx.db,
                    session_id,
                    &user_id,
                    payload,
                )
                .await?;
            }
        }
    }
    interaction
        .create_response(
            &ctx.http,
            serenity::builder::CreateInteractionResponse::Acknowledge,
        )
        .await?;
    Ok(())
}

pub async fn handle_wizard_submit_tag_select_change(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    if let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind {
        if let Some(tag) = values.first() {
            let user_id = interaction.user.id.get().to_string();
            if let Some(mut payload) =
                crate::db::get_workflow_session_payload(&app_ctx.db, session_id, &user_id).await?
            {
                payload["selected_tag"] = serde_json::Value::String(tag.clone());
                crate::db::update_workflow_session_payload(
                    &app_ctx.db,
                    session_id,
                    &user_id,
                    payload,
                )
                .await?;
            }
        }
    }
    interaction
        .create_response(
            &ctx.http,
            serenity::builder::CreateInteractionResponse::Acknowledge,
        )
        .await?;
    Ok(())
}

pub async fn handle_wizard_events_edit(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id_arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let mut selected_uid = String::new();
    if let Some(sid) = session_id_arg {
        let user_id = interaction.user.id.get().to_string();
        if let Some(payload) =
            crate::db::get_workflow_session_payload(&app_ctx.db, sid, &user_id).await?
        {
            if let Some(uid) = payload.get("selected_event_uid").and_then(|v| v.as_str()) {
                selected_uid = uid.to_string();
            }
        }
    }

    if selected_uid.is_empty() {
        let resp = serenity::builder::CreateInteractionResponseMessage::new().content(
            rust_i18n::t!("errors.no_events_to_edit", locale = locale.as_str()),
        );
        interaction
            .create_response(
                ctx,
                serenity::builder::CreateInteractionResponse::Message(resp),
            )
            .await?;
        return Ok(());
    }

    if let Some(pb_cfg) = &app_ctx.config.pocketbase {
        if let Ok(events) = crate::events_sync::fetch_pocketbase_events(
            app_ctx.http.as_ref(),
            pb_cfg,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            if let Some(ev) = events.into_iter().find(|e| e.uid == selected_uid) {
                let current_tag = ev
                    .tags
                    .as_ref()
                    .and_then(|t| t.first())
                    .map_or("", std::string::String::as_str);

                let user_id = interaction.user.id.get().to_string();
                let guild_id = interaction
                    .guild_id
                    .map(|g| g.get().to_string())
                    .unwrap_or_default();
                let channel_id = interaction.channel_id.get().to_string();

                let session_payload = serde_json::json!({
                    "uid": selected_uid,
                    "tag": current_tag
                });

                let session_id = crate::db::create_workflow_session(
                    &app_ctx.db,
                    "edit_event",
                    &user_id,
                    &guild_id,
                    &channel_id,
                    session_payload,
                    15,
                )
                .await?;

                let start_str = ev
                    .start_time
                    .with_timezone(&chrono_tz::Europe::Helsinki)
                    .format("%Y-%m-%d %H:%M")
                    .to_string();
                let end_str = ev
                    .end_time
                    .map(|d| {
                        d.with_timezone(&chrono_tz::Europe::Helsinki)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();

                let mut safe_desc = ev.description.clone().unwrap_or_default();
                if safe_desc.len() > 1000 {
                    let mut cutoff = 997;
                    while !safe_desc.is_char_boundary(cutoff) && cutoff > 0 {
                        cutoff -= 1;
                    }
                    safe_desc.truncate(cutoff);
                    safe_desc.push_str("...");
                }

                let dates_str = if end_str.is_empty() {
                    start_str.clone()
                } else {
                    format!("{start_str} - {end_str}")
                };

                let modal = serenity::builder::CreateModal::new(
                    format!("modal_edit_event_{session_id}"),
                    "Edit Event",
                )
                .components(vec![
                    serenity::builder::CreateActionRow::InputText(
                        serenity::builder::CreateInputText::new(
                            serenity::all::InputTextStyle::Short,
                            "Title",
                            "title",
                        )
                        .value(&ev.summary)
                        .required(true),
                    ),
                    serenity::builder::CreateActionRow::InputText(
                        serenity::builder::CreateInputText::new(
                            serenity::all::InputTextStyle::Short,
                            "Dates (YYYY-MM-DD HH:MM - HH:MM)",
                            "dates",
                        )
                        .value(&dates_str)
                        .required(true),
                    ),
                    serenity::builder::CreateActionRow::InputText(
                        serenity::builder::CreateInputText::new(
                            serenity::all::InputTextStyle::Short,
                            "Location",
                            "location",
                        )
                        .value(ev.location.as_deref().unwrap_or_default())
                        .required(false),
                    ),
                    serenity::builder::CreateActionRow::InputText(
                        serenity::builder::CreateInputText::new(
                            serenity::all::InputTextStyle::Short,
                            "URL",
                            "url",
                        )
                        .value(ev.url.as_deref().unwrap_or_default())
                        .required(false),
                    ),
                    serenity::builder::CreateActionRow::InputText(
                        serenity::builder::CreateInputText::new(
                            serenity::all::InputTextStyle::Paragraph,
                            "Description",
                            "description",
                        )
                        .value(&safe_desc)
                        .required(false),
                    ),
                ]);

                interaction
                    .create_response(
                        &ctx.http,
                        serenity::builder::CreateInteractionResponse::Modal(modal),
                    )
                    .await?;

                let msg =
                    rust_i18n::t!("command.events.opening_edit_form", locale = locale.as_str());
                let _ = interaction
                    .edit_response(
                        &ctx.http,
                        serenity::builder::EditInteractionResponse::new()
                            .content(msg.as_ref())
                            .components(vec![]),
                    )
                    .await;
                return Ok(());
            }
        }
    }

    let resp = serenity::builder::CreateInteractionResponseMessage::new().content(rust_i18n::t!(
        "errors.failed_to_fetch_events",
        locale = locale.as_str()
    ));
    interaction
        .create_response(
            ctx,
            serenity::builder::CreateInteractionResponse::Message(resp),
        )
        .await?;
    Ok(())
}

pub async fn handle_modal_edit_event(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ModalInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;
    let mut title = String::new();
    let mut dates_str = String::new();
    let mut location = String::new();
    let mut url = String::new();
    let mut description = String::new();

    let user_id = interaction.user.id.get().to_string();
    let guild_id = interaction
        .guild_id
        .map(|g| g.get().to_string())
        .unwrap_or_default();

    let Some(session_payload) = crate::db::authorize_and_consume_session(
        &app_ctx.db,
        session_id,
        "edit_event",
        &user_id,
        &guild_id,
    )
    .await?
    else {
        let resp = CreateInteractionResponseMessage::new()
            .content(rust_i18n::t!(
                "errors.session_consumed",
                locale = locale.as_str()
            ))
            .ephemeral(true);
        let _ = interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
            .await;
        return Ok(());
    };

    let uid = session_payload
        .get("uid")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let tags = session_payload
        .get("tag")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    for row in &interaction.data.components {
        if let Some(ActionRowComponent::InputText(input)) = row.components.first() {
            match input.custom_id.as_str() {
                "title" => title = input.value.clone().unwrap_or_default(),
                "dates" => dates_str = input.value.clone().unwrap_or_default(),
                "location" => location = input.value.clone().unwrap_or_default(),
                "url" => url = input.value.clone().unwrap_or_default(),
                "description" => description = input.value.clone().unwrap_or_default(),
                _ => {}
            }
        }
    }

    let (pb_start, pb_end) = match validate_event_dates(&dates_str) {
        Ok(dates) => dates,
        Err(e) => {
            let draft_id = uuid::Uuid::new_v4().to_string();
            let draft_payload = serde_json::json!({
                "title": title,
                "dates": dates_str,
                "location": location,
                "url": url,
                "tag": tags,
                "description": description,
                "is_edit": true,
                "uid": uid,
            });

            let _ = crate::db::insert_draft_submission(
                &app_ctx.db,
                &draft_id,
                &interaction.user.id.to_string(),
                &interaction.channel_id.to_string(),
                &draft_payload.to_string(),
            )
            .await;

            let msg = rust_i18n::t!(
                "errors.validation_failed",
                locale = locale.as_str(),
                e = e.clone()
            );
            let resp = CreateInteractionResponseMessage::new()
                .content(msg)
                .components(vec![CreateActionRow::Buttons(vec![
                    CreateButton::new(format!("wizard_retry_submit:{draft_id}"))
                        .label(rust_i18n::t!(
                            "command.events.edit_and_retry",
                            locale = locale.as_str()
                        ))
                        .style(ButtonStyle::Primary),
                ])])
                .ephemeral(true);
            let _ = interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
            return Ok(());
        }
    };

    let mut tags_array = vec![];
    if !tags.is_empty() {
        tags_array.push(tags);
    }
    let payload_json = serde_json::json!({
        "uid": uid,
        "title": title,
        "start_date": pb_start,
        "end_date": pb_end,
        "location": location,
        "url": url,
        "tags": tags_array,
        "description": description,
    });
    let payload_str = payload_json.to_string();

    let mut diff_str = String::new();
    if let Some(pb_cfg) = &app_ctx.config.pocketbase {
        if let Ok(events) = crate::events_sync::fetch_pocketbase_events(
            app_ctx.http.as_ref(),
            pb_cfg,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            if let Some(ev) = events.into_iter().find(|e| e.uid == uid) {
                let old_payload = serde_json::json!({
                    "uid": ev.uid,
                    "title": ev.summary,
                    "start_date": ev.start_time.with_timezone(&Helsinki).format("%Y-%m-%d %H:%M:00").to_string(),
                    "end_date": ev.end_time.map_or_else(String::new, |d| d.with_timezone(&Helsinki).format("%Y-%m-%d %H:%M:00").to_string()),
                    "location": ev.location.unwrap_or_default(),
                    "url": ev.url.unwrap_or_default(),
                    "tags": ev.tags.unwrap_or_default(),
                    "description": ev.description.unwrap_or_default(),
                });
                let old_yaml = generate_display_yaml(&old_payload);
                let new_yaml = generate_display_yaml(&payload_json);

                let diff = similar::TextDiff::from_lines(&old_yaml, &new_yaml);
                for change in diff.iter_all_changes() {
                    let sign = match change.tag() {
                        similar::ChangeTag::Delete => "-",
                        similar::ChangeTag::Insert => "+",
                        similar::ChangeTag::Equal => " ",
                    };
                    diff_str.push_str(&format!("{sign}{change}"));
                }
            }
        }
    }
    let yaml_str = generate_display_yaml(&payload_json);

    if diff_str.is_empty() {
        diff_str = yaml_str.clone();
    }

    let user_id = interaction.user.id.get().to_string();
    let channel_id = interaction.channel_id.get().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let topic_prefix = rust_i18n::t!("command.zulip.topic_edit_event", locale = locale.as_str());
    let start_date = pb_start.clone();
    let zulip_topic = format_zulip_topic(&topic_prefix, &start_date, &title, &uid);

    if let Some(zulip_cfg) = &app_ctx.config.zulip {
        match crate::db::insert_input_submission(
            &app_ctx.db,
            &id,
            &user_id,
            &channel_id,
            &zulip_cfg.moderation_stream,
            &zulip_topic,
            &payload_str,
        )
        .await
        {
            Ok(()) => {
                let cmd_approve =
                    rust_i18n::t!("command.zulip.cmd_approve", locale = locale.as_str());
                let cmd_reject =
                    rust_i18n::t!("command.zulip.cmd_reject", locale = locale.as_str());

                let content = rust_i18n::t!(
                    "command.zulip.edit_event",
                    locale = locale.as_str(),
                    uid = uid,
                    title = title,
                    start_date = pb_start,
                    end_date = pb_end,
                    location = location,
                    description = description,
                    payload_str = diff_str,
                    yaml_str = yaml_str,
                    cmd_approve = cmd_approve,
                    cmd_reject = cmd_reject
                )
                .to_string();

                let _ = crate::zulip::post_topic_to_stream(
                    app_ctx.http.as_ref(),
                    zulip_cfg,
                    &zulip_cfg.moderation_stream,
                    &zulip_topic,
                    &content,
                    app_ctx.config.resource_limits.max_http_body_bytes,
                )
                .await;

                let msg = rust_i18n::t!("command.events.edit_received", locale = locale.as_str());
                let final_msg = if msg.is_empty() || msg == "command.events.edit_received" {
                    "Your event edit has been submitted for moderation review."
                } else {
                    &msg
                };
                let resp = CreateInteractionResponseMessage::new()
                    .content(final_msg)
                    .ephemeral(true);
                let _ = interaction
                    .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                    .await;
            }
            Err(e) => {
                tracing::error!("DB insert error: {}", e);
                let msg = rust_i18n::t!("command.events.edit_failed", locale = locale.as_str());
                let final_msg = if msg.is_empty() || msg == "command.events.edit_failed" {
                    "Failed to submit event edit."
                } else {
                    &msg
                };
                let resp = CreateInteractionResponseMessage::new()
                    .content(final_msg)
                    .ephemeral(true);
                let _ = interaction
                    .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                    .await;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_submit_modal_structure() {
        let tag = "exhibition";
        let modal = create_submit_modal(tag);

        let json = serde_json::to_value(&modal).unwrap();

        // Assert custom_id contains tag
        let custom_id = json["custom_id"].as_str().unwrap();
        assert_eq!(custom_id, format!("modal_submit_event_{tag}"));

        // Assert exactly 5 components
        let components = json["components"].as_array().unwrap();
        assert_eq!(components.len(), 5);

        // Extract component IDs
        let mut ids = vec![];
        for row in components {
            let row_comps = row["components"].as_array().unwrap();
            let comp = &row_comps[0];
            ids.push(comp["custom_id"].as_str().unwrap().to_string());
        }

        assert_eq!(
            ids,
            vec!["title", "dates", "location", "url", "description"]
        );

        let labels: Vec<_> = components
            .iter()
            .map(|row| row["components"][0]["label"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            vec![
                "Title",
                "Dates (YYYY-MM-DD HH:MM - HH:MM)",
                "Location",
                "URL",
                "Description"
            ]
        );
    }
}
