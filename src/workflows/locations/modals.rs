use crate::workflows::events::{validate_text_field, validate_url};
use crate::workflows::locations::models::{GeoPoint, Location, LocationModerationPayload};
use crate::workflows::locations::pocketbase::fetch_pocketbase_locations;
use crate::workflows::locations::validation::{format_location_zulip_topic, sanitize_tags};
use crate::workflows::{AppContext, extract_locale_and_bot_name};
use serde::Serialize;
use serenity::all::{
    ActionRowComponent, ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, Context,
    CreateActionRow, CreateButton, CreateInputText, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateModal, InputTextStyle, ModalInteraction,
};

const MAX_TITLE_LEN: usize = 100;
const MAX_ADDRESS_LEN: usize = 200;
const MAX_OPENING_HOURS_LEN: usize = 200;
const MAX_DESCRIPTION_LEN: usize = 2000;

/// Point as moderators see it: `null` coordinates are an explicit, fillable slot.
#[derive(Serialize)]
struct DisplayPoint {
    lat: Option<f64>,
    lon: Option<f64>,
}

#[derive(Serialize)]
struct LocationDisplayPayload<'a> {
    title: &'a str,
    location: &'a str,
    opening_hours: &'a str,
    description: String,
    url: &'a str,
    tags: Vec<String>,
    point: DisplayPoint,
}

/// Renders the YAML block moderators edit in Zulip.
///
/// Unlike the events variant this keeps `tags` as a real YAML list (the collection allows two)
/// and always emits a `point` block so a missing geoPoint is visible and fillable.
#[must_use]
pub fn generate_location_display_yaml(payload: &serde_json::Value) -> String {
    let get_str = |key: &str| payload.get(key).and_then(|v| v.as_str()).unwrap_or("");

    let mut description = get_str("description").replace('\r', "");
    if !description.ends_with('\n') {
        description.push('\n');
    }

    let tags = payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let point = crate::workflows::locations::validation::extract_point(payload);

    let display_obj = LocationDisplayPayload {
        title: get_str("title"),
        location: get_str("location"),
        opening_hours: get_str("opening_hours"),
        description,
        url: get_str("url"),
        tags,
        point: DisplayPoint {
            lat: point.map(|p| p.lat),
            lon: point.map(|p| p.lon),
        },
    };

    serde_yaml::to_string(&display_obj)
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// Builds a unified diff between the live `PocketBase` record and the proposed YAML.
pub async fn generate_location_diff_str(
    pb_cfg: Option<&crate::config::PocketBaseConfig>,
    http: &dyn crate::http::HttpProvider,
    limit: u64,
    uid: &str,
    new_yaml: &str,
) -> String {
    let mut diff_str = String::new();
    if let Some(pb_cfg) = pb_cfg {
        if let Ok(locations) = fetch_pocketbase_locations(http, pb_cfg, limit).await {
            if let Some(loc) = locations.into_iter().find(|l| l.uid == uid) {
                let old_yaml = generate_location_display_yaml(&location_to_payload(&loc));

                let diff = similar::TextDiff::from_lines(old_yaml.as_str(), new_yaml);
                diff_str.push_str("--- original\n+++ edited\n");
                for change in diff.iter_all_changes() {
                    let sign = match change.tag() {
                        similar::ChangeTag::Delete => "-",
                        similar::ChangeTag::Insert => "+",
                        similar::ChangeTag::Equal => " ",
                    };
                    diff_str.push_str(&format!("{sign}{change}"));
                }
                diff_str = diff_str.trim_end().to_string();
            }
        }
    }

    if diff_str.is_empty() {
        diff_str = format!("--- original (not found)\n+++ edited\n{new_yaml}");
    }

    diff_str
}

/// Flattens a [`Location`] into the JSON shape the display/diff helpers expect.
#[must_use]
pub fn location_to_payload(loc: &Location) -> serde_json::Value {
    serde_json::json!({
        "uid": loc.uid,
        "title": loc.title,
        "location": loc.location.clone().unwrap_or_default(),
        "opening_hours": loc.opening_hours.clone().unwrap_or_default(),
        "description": loc.description.clone().unwrap_or_default(),
        "url": loc.url.clone().unwrap_or_default(),
        "tags": loc.tags.clone().unwrap_or_default(),
        "point": loc.point,
    })
}

/// The five-field submission modal. Coordinates are deliberately absent — moderators set
/// `point` from the Zulip YAML block.
pub fn create_submit_location_modal(session_id: &str) -> CreateModal {
    build_location_modal(
        &format!("modal_submit_location_{session_id}"),
        "Submit a Location",
        &LocationFormValues::default(),
    )
}

/// Values used to prefill the modal on edit or retry.
#[derive(Default, Clone, Debug)]
pub struct LocationFormValues {
    pub title: String,
    pub location: String,
    pub opening_hours: String,
    pub url: String,
    pub description: String,
}

fn build_location_modal(
    custom_id: &str,
    modal_title: &str,
    values: &LocationFormValues,
) -> CreateModal {
    CreateModal::new(custom_id, modal_title).components(vec![
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "Title", "title")
                .required(true)
                .value(&values.title),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "Address", "location")
                .required(false)
                .value(&values.location),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "Opening hours", "opening_hours")
                .required(false)
                .value(&values.opening_hours),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "URL", "url")
                .required(false)
                .value(&values.url),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Paragraph, "Description", "description")
                .required(false)
                .value(&values.description),
        ),
    ])
}

fn read_location_modal(interaction: &ModalInteraction) -> LocationFormValues {
    let mut values = LocationFormValues::default();
    for row in &interaction.data.components {
        if let Some(ActionRowComponent::InputText(input)) = row.components.first() {
            let value = input.value.clone().unwrap_or_default();
            match input.custom_id.as_str() {
                "title" => values.title = value,
                "location" => values.location = value,
                "opening_hours" => values.opening_hours = value,
                "url" => values.url = value,
                "description" => values.description = value,
                _ => {}
            }
        }
    }
    values
}

/// Runs every field through the shared validators, collecting all failures at once.
fn validate_location_form(values: &LocationFormValues) -> Result<LocationFormValues, Vec<String>> {
    let title = validate_text_field(&values.title, "Title", MAX_TITLE_LEN, true);
    let location = validate_text_field(&values.location, "Address", MAX_ADDRESS_LEN, false);
    let opening_hours = validate_text_field(
        &values.opening_hours,
        "Opening hours",
        MAX_OPENING_HOURS_LEN,
        false,
    );
    let url = validate_url(&values.url);
    let description = validate_text_field(
        &values.description,
        "Description",
        MAX_DESCRIPTION_LEN,
        false,
    );

    let errors: Vec<String> = [
        title.as_ref().err(),
        location.as_ref().err(),
        opening_hours.as_ref().err(),
        url.as_ref().err(),
        description.as_ref().err(),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();

    if errors.is_empty() {
        Ok(LocationFormValues {
            title: title.unwrap(),
            location: location.unwrap(),
            opening_hours: opening_hours.unwrap(),
            url: url.unwrap(),
            description: description.unwrap(),
        })
    } else {
        Err(errors)
    }
}

async fn respond_with_validation_errors(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ModalInteraction,
    locale: &str,
    errors: &[String],
    draft_payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let draft_id = uuid::Uuid::new_v4().to_string();
    let _ = crate::db::insert_draft_submission(
        &app_ctx.db,
        &draft_id,
        &interaction.user.id.to_string(),
        &interaction.channel_id.to_string(),
        &draft_payload.to_string(),
    )
    .await;

    let retry_session_id = crate::db::create_workflow_session(
        &app_ctx.db,
        "retry_location_submit",
        &interaction.user.id.to_string(),
        &interaction
            .guild_id
            .map(|g| g.get().to_string())
            .unwrap_or_default(),
        &interaction.channel_id.to_string(),
        serde_json::json!({ "draft_id": draft_id }),
        15,
    )
    .await?;

    let msg = rust_i18n::t!(
        "errors.validation_failed",
        locale = locale,
        e = errors.join("\n")
    );
    let resp = CreateInteractionResponseMessage::new()
        .content(msg)
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("wizard_retry_location_submit:{retry_session_id}"))
                .label(rust_i18n::t!(
                    "command.locations.edit_and_retry",
                    locale = locale
                ))
                .style(ButtonStyle::Primary),
        ])])
        .ephemeral(true);
    let _ = interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
        .await;
    Ok(())
}

/// Stores the moderator-visible tag selection on the wizard session.
pub async fn handle_wizard_location_tag_select_change(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    if let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind {
        let tags = sanitize_tags(values);
        let user_id = interaction.user.id.get().to_string();
        if let Some(mut payload) =
            crate::db::get_workflow_session_payload(&app_ctx.db, session_id, &user_id).await?
        {
            payload["selected_tags"] = serde_json::json!(tags);
            crate::db::update_workflow_session_payload(&app_ctx.db, session_id, &user_id, payload)
                .await?;
        }
    }
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;
    Ok(())
}

/// Stores which existing location the "propose changes" button will open.
pub async fn handle_wizard_edit_location_select_change(
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
                payload["selected_location_uid"] = serde_json::Value::String(uid.clone());
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
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;
    Ok(())
}

/// Consumes the wizard session and opens the empty submission modal.
pub async fn handle_wizard_location_submit_continue(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id_arg: Option<&str>,
) -> anyhow::Result<()> {
    let mut tags: Vec<String> = vec!["other".to_string()];
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
            "main_locations_wizard",
            &user_id,
            &guild_id,
        )
        .await?;
        if let Some(p) = payload_opt {
            draft_id_opt = p
                .get("draft_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if let Some(selected) = p.get("selected_tags").and_then(|v| v.as_array()) {
                let parsed: Vec<String> = selected
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                let sanitized = sanitize_tags(&parsed);
                if !sanitized.is_empty() {
                    tags = sanitized;
                }
            }
        }
    }

    let session_id = crate::db::create_workflow_session(
        &app_ctx.db,
        "submit_location",
        &user_id,
        &guild_id,
        &channel_id,
        serde_json::json!({ "draft_id": draft_id_opt, "tags": tags }),
        15,
    )
    .await?;

    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Modal(create_submit_location_modal(&session_id)),
        )
        .await?;

    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;
    let msg = rust_i18n::t!(
        "command.locations.opening_submit_form",
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

/// Handles the new-location modal: validates, then opens a Zulip moderation topic.
pub async fn handle_modal_submit_location(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    if let Some(msg) = interaction.message.as_ref() {
        let _ = msg.delete(&ctx.http).await;
    }

    let Some(session_id) = interaction
        .data
        .custom_id
        .strip_prefix("modal_submit_location_")
    else {
        return Ok(());
    };

    let user_id = interaction.user.id.get().to_string();
    let guild_id = interaction
        .guild_id
        .map(|g| g.get().to_string())
        .unwrap_or_default();

    let Some(session_payload) = crate::db::authorize_and_consume_session(
        &app_ctx.db,
        session_id,
        "submit_location",
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

    let tags: Vec<String> = session_payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let draft_uuid = session_payload
        .get("draft_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let raw = read_location_modal(interaction);
    let values = match validate_location_form(&raw) {
        Ok(v) => v,
        Err(errors) => {
            let draft_payload = serde_json::json!({
                "title": raw.title,
                "location": raw.location,
                "opening_hours": raw.opening_hours,
                "url": raw.url,
                "description": raw.description,
                "tags": tags,
                "is_edit": false,
                "uid": null,
            });
            return respond_with_validation_errors(
                ctx,
                app_ctx,
                interaction,
                locale.as_str(),
                &errors,
                &draft_payload,
            )
            .await;
        }
    };

    let mut image_url: Option<String> = None;
    if let Some(ref uuid) = draft_uuid {
        if let Some(json_str) = crate::db::get_drafting_payload(&app_ctx.db, uuid).await? {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_str) {
                image_url = json
                    .get("image_url")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
            }
            let _ = crate::db::delete_input(&app_ctx.db, uuid).await;
        }
    }

    let generated_uid = uuid::Uuid::new_v4().as_simple().to_string()[..15].to_string();

    let mod_payload = LocationModerationPayload {
        id: generated_uid.clone(),
        title: values.title.clone(),
        location: values.location.clone(),
        opening_hours: values.opening_hours.clone(),
        description: values.description.clone(),
        url: values.url.clone(),
        tags: sanitize_tags(&tags),
        state: "draft".to_string(),
        point: None,
        image_url: image_url.clone(),
    };

    let payload_value = serde_json::to_value(&mod_payload)?;
    let payload_str = serde_json::to_string(&mod_payload)?;

    let topic_prefix = rust_i18n::t!("command.zulip.topic_new_location", locale = locale.as_str());
    let zulip_topic = format_location_zulip_topic(&topic_prefix, &values.title, &generated_uid);

    let Some(zulip_cfg) = &app_ctx.config.zulip else {
        return Ok(());
    };

    let display_payload = generate_location_display_yaml(&payload_value);
    let cmd_approve = rust_i18n::t!("command.zulip.cmd_approve", locale = locale.as_str());
    let cmd_approve_published = rust_i18n::t!(
        "command.zulip.cmd_approve_published",
        locale = locale.as_str()
    );
    let cmd_reject = rust_i18n::t!("command.zulip.cmd_reject", locale = locale.as_str());

    let content = image_url.map_or_else(
        || {
            rust_i18n::t!(
                "command.zulip.new_location_no_image",
                locale = locale.as_str(),
                title = values.title,
                location = values.location,
                opening_hours = values.opening_hours,
                description = values.description,
                payload_str = display_payload,
                cmd_approve = cmd_approve,
                cmd_approve_published = cmd_approve_published,
                cmd_reject = cmd_reject
            )
            .to_string()
        },
        |url| {
            rust_i18n::t!(
                "command.zulip.new_location",
                locale = locale.as_str(),
                title = values.title,
                location = values.location,
                opening_hours = values.opening_hours,
                description = values.description,
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
        &uuid::Uuid::new_v4().to_string(),
        &user_id,
        &interaction.channel_id.get().to_string(),
        &zulip_cfg.moderation_stream,
        &zulip_topic,
        &payload_str,
        &outbox_id,
        &outbox_body,
    )
    .await;

    let resp = CreateInteractionResponseMessage::new()
        .content(rust_i18n::t!(
            "command.locations.submission_received",
            locale = locale.as_str()
        ))
        .ephemeral(true);
    let _ = interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
        .await;

    Ok(())
}

/// Opens the edit modal prefilled from the location selected in the wizard.
pub async fn handle_wizard_locations_edit(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id_arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let mut selected_uid = String::new();
    let mut locations_cache: Vec<Location> = Vec::new();

    if let Some(sid) = session_id_arg {
        let user_id = interaction.user.id.get().to_string();
        if let Some(payload) =
            crate::db::get_workflow_session_payload(&app_ctx.db, sid, &user_id).await?
        {
            if let Some(uid) = payload
                .get("selected_location_uid")
                .and_then(|v| v.as_str())
            {
                selected_uid = uid.to_string();
            }
            if let Some(cache) = payload.get("locations_cache") {
                if let Ok(parsed) = serde_json::from_value(cache.clone()) {
                    locations_cache = parsed;
                }
            }
        }
    }

    let selected = (!selected_uid.is_empty())
        .then(|| locations_cache.into_iter().find(|l| l.uid == selected_uid))
        .flatten();

    let Some(loc) = selected else {
        let resp = CreateInteractionResponseMessage::new().content(rust_i18n::t!(
            "errors.no_locations_to_edit",
            locale = locale.as_str()
        ));
        interaction
            .create_response(ctx, CreateInteractionResponse::Message(resp))
            .await?;
        return Ok(());
    };

    let session_id = crate::db::create_workflow_session(
        &app_ctx.db,
        "edit_location",
        &interaction.user.id.get().to_string(),
        &interaction
            .guild_id
            .map(|g| g.get().to_string())
            .unwrap_or_default(),
        &interaction.channel_id.get().to_string(),
        serde_json::json!({
            "uid": selected_uid,
            "tags": loc.tags.clone().unwrap_or_default(),
        }),
        15,
    )
    .await?;

    let mut description = loc.description.clone().unwrap_or_default();
    if description.len() > 1000 {
        let mut cutoff = 997;
        while !description.is_char_boundary(cutoff) && cutoff > 0 {
            cutoff -= 1;
        }
        description.truncate(cutoff);
        description.push_str("...");
    }

    let modal = build_location_modal(
        &format!("modal_edit_location_{session_id}"),
        "Edit Location",
        &LocationFormValues {
            title: loc.title.clone(),
            location: loc.location.clone().unwrap_or_default(),
            opening_hours: loc.opening_hours.clone().unwrap_or_default(),
            url: loc.url.clone().unwrap_or_default(),
            description,
        },
    );

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;

    let msg = rust_i18n::t!(
        "command.locations.opening_edit_form",
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

/// Handles the edit modal: posts a diff to the location's Zulip moderation topic.
pub async fn handle_modal_edit_location(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ModalInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let user_id = interaction.user.id.get().to_string();
    let guild_id = interaction
        .guild_id
        .map(|g| g.get().to_string())
        .unwrap_or_default();

    let Some(session_payload) = crate::db::authorize_and_consume_session(
        &app_ctx.db,
        session_id,
        "edit_location",
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
    let tags: Vec<String> = session_payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let raw = read_location_modal(interaction);
    let values = match validate_location_form(&raw) {
        Ok(v) => v,
        Err(errors) => {
            let draft_payload = serde_json::json!({
                "title": raw.title,
                "location": raw.location,
                "opening_hours": raw.opening_hours,
                "url": raw.url,
                "description": raw.description,
                "tags": tags,
                "is_edit": true,
                "uid": uid,
            });
            return respond_with_validation_errors(
                ctx,
                app_ctx,
                interaction,
                locale.as_str(),
                &errors,
                &draft_payload,
            )
            .await;
        }
    };

    // `point` and `state` are intentionally absent: PocketBase PATCHes partially, so the
    // existing coordinates and publication state survive an edit.
    let payload_json = serde_json::json!({
        "uid": uid,
        "title": values.title,
        "location": values.location,
        "opening_hours": values.opening_hours,
        "url": values.url,
        "tags": sanitize_tags(&tags),
        "description": values.description,
    });

    let yaml_str = generate_location_display_yaml(&payload_json);
    let diff_str = generate_location_diff_str(
        app_ctx.config.pocketbase.as_ref(),
        app_ctx.http.as_ref(),
        app_ctx.config.resource_limits.max_http_body_bytes,
        &uid,
        &yaml_str,
    )
    .await;

    let topic_prefix = rust_i18n::t!(
        "command.zulip.topic_edit_location",
        locale = locale.as_str()
    );
    let zulip_topic = format_location_zulip_topic(&topic_prefix, &values.title, &uid);

    let Some(zulip_cfg) = &app_ctx.config.zulip else {
        return Ok(());
    };

    let limit = app_ctx.config.resource_limits.max_http_body_bytes;
    match crate::db::insert_input_submission(
        &app_ctx.db,
        &uuid::Uuid::new_v4().to_string(),
        &user_id,
        &interaction.channel_id.get().to_string(),
        &zulip_cfg.moderation_stream,
        &zulip_topic,
        &payload_json.to_string(),
    )
    .await
    {
        Ok(()) => {
            let cmd_approve = rust_i18n::t!("command.zulip.cmd_approve", locale = locale.as_str());
            let cmd_reject = rust_i18n::t!("command.zulip.cmd_reject", locale = locale.as_str());

            let content = rust_i18n::t!(
                "command.zulip.edit_location",
                locale = locale.as_str(),
                uid = uid,
                title = values.title,
                location = values.location,
                opening_hours = values.opening_hours,
                description = values.description,
                payload_str = diff_str,
                yaml_str = yaml_str,
                cmd_approve = cmd_approve,
                cmd_reject = cmd_reject
            )
            .to_string();

            let _ = crate::zulip::unresolve_zulip_topic(
                app_ctx.http.as_ref(),
                zulip_cfg,
                &zulip_topic,
                limit,
            )
            .await;

            let _ = crate::zulip::post_topic_to_stream(
                app_ctx.http.as_ref(),
                zulip_cfg,
                &zulip_cfg.moderation_stream,
                &zulip_topic,
                &content,
                limit,
            )
            .await;

            let resp = CreateInteractionResponseMessage::new()
                .content(rust_i18n::t!(
                    "command.locations.edit_received",
                    locale = locale.as_str()
                ))
                .ephemeral(true);
            let _ = interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
        Err(e) => {
            tracing::error!("DB insert error for location edit: {}", e);
            let resp = CreateInteractionResponseMessage::new()
                .content(rust_i18n::t!(
                    "command.locations.edit_failed",
                    locale = locale.as_str()
                ))
                .ephemeral(true);
            let _ = interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                .await;
        }
    }

    Ok(())
}

/// Reopens the modal, prefilled from the draft stored when validation failed.
pub async fn handle_wizard_retry_location_submit(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id_arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let Some(sid) = session_id_arg else {
        return Ok(());
    };

    let user_id = interaction.user.id.get().to_string();
    let guild_id = interaction
        .guild_id
        .map(|g| g.get().to_string())
        .unwrap_or_default();
    let channel_id = interaction.channel_id.get().to_string();

    let Some(session_payload) = crate::db::authorize_and_consume_session(
        &app_ctx.db,
        sid,
        "retry_location_submit",
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

    let Some(draft_id) = session_payload.get("draft_id").and_then(|v| v.as_str()) else {
        return Ok(());
    };
    let Ok(payload_str) = crate::db::get_draft_submission(&app_ctx.db, draft_id).await else {
        return Ok(());
    };
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_str) else {
        return Ok(());
    };

    let get_str = |key: &str| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let is_edit = payload
        .get("is_edit")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let uid = get_str("uid");
    let tags: Vec<String> = payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let session_id = crate::db::create_workflow_session(
        &app_ctx.db,
        if is_edit {
            "edit_location"
        } else {
            "submit_location"
        },
        &user_id,
        &guild_id,
        &channel_id,
        serde_json::json!({
            "draft_id": if is_edit { None } else { Some(draft_id) },
            "uid": if is_edit { Some(uid) } else { None },
            "tags": tags,
        }),
        15,
    )
    .await?;

    let modal = build_location_modal(
        &format!(
            "{}{session_id}",
            if is_edit {
                "modal_edit_location_"
            } else {
                "modal_submit_location_"
            }
        ),
        if is_edit {
            "Edit Location"
        } else {
            "Submit a Location"
        },
        &LocationFormValues {
            title: get_str("title"),
            location: get_str("location"),
            opening_hours: get_str("opening_hours"),
            url: get_str("url"),
            description: get_str("description"),
        },
    );

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;

    let msg = rust_i18n::t!(
        "command.locations.opening_edit_form",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_location_display_yaml_format() {
        let payload = serde_json::json!({
            "id": "abcdefghij12345",
            "title": "Legokauppa",
            "location": "Mannerheimintie 1, Helsinki",
            "opening_hours": "ma-pe 10-18",
            "description": "Iso valikoima\r\nsettejä",
            "url": "https://example.com",
            "tags": ["store", "museum"],
            "state": "draft"
        });

        let yaml = generate_location_display_yaml(&payload);

        // Field order is the order moderators read.
        let title_at = yaml.find("title:").unwrap();
        let location_at = yaml.find("location:").unwrap();
        let hours_at = yaml.find("opening_hours:").unwrap();
        let desc_at = yaml.find("description:").unwrap();
        let url_at = yaml.find("url:").unwrap();
        let tags_at = yaml.find("tags:").unwrap();
        let point_at = yaml.find("point:").unwrap();
        assert!(title_at < location_at);
        assert!(location_at < hours_at);
        assert!(hours_at < desc_at);
        assert!(desc_at < url_at);
        assert!(url_at < tags_at);
        assert!(tags_at < point_at);

        // Internal ids and state stay out of the moderator-facing block.
        assert!(!yaml.contains("abcdefghij12345"));
        assert!(!yaml.contains("state:"));

        // Carriage returns are stripped so the YAML block renders cleanly.
        assert!(!yaml.contains('\r'));

        // Tags stay a real list rather than the events `type` scalar.
        assert!(yaml.contains("- store"));
        assert!(yaml.contains("- museum"));
        assert!(!yaml.contains("type:"));
    }

    #[test]
    fn test_generate_location_display_yaml_emits_point_placeholder() {
        let payload = serde_json::json!({ "title": "Museo" });
        let yaml = generate_location_display_yaml(&payload);

        assert!(yaml.contains("point:"));
        assert!(yaml.contains("lat: null"));
        assert!(yaml.contains("lon: null"));
    }

    #[test]
    fn test_generate_location_display_yaml_preserves_point() {
        let payload = serde_json::json!({
            "title": "Museo",
            "point": {"lat": 60.1699, "lon": 24.9384}
        });
        let yaml = generate_location_display_yaml(&payload);

        assert!(yaml.contains("lat: 60.1699"));
        assert!(yaml.contains("lon: 24.9384"));
    }

    #[test]
    fn test_create_submit_location_modal_structure() {
        let modal = create_submit_location_modal("session123");
        let json = serde_json::to_value(&modal).unwrap();

        assert_eq!(
            json.get("custom_id").unwrap().as_str().unwrap(),
            "modal_submit_location_session123"
        );

        let components = json.get("components").unwrap().as_array().unwrap();
        assert_eq!(components.len(), 5);

        let ids: Vec<&str> = components
            .iter()
            .map(|row| {
                row.get("components").unwrap().as_array().unwrap()[0]
                    .get("custom_id")
                    .unwrap()
                    .as_str()
                    .unwrap()
            })
            .collect();
        assert_eq!(
            ids,
            vec!["title", "location", "opening_hours", "url", "description"]
        );
    }

    #[test]
    fn test_validate_location_form_collects_all_errors() {
        let values = LocationFormValues {
            title: String::new(),
            location: "a".repeat(MAX_ADDRESS_LEN + 1),
            opening_hours: "a".repeat(MAX_OPENING_HOURS_LEN + 1),
            url: "not-a-url".to_string(),
            description: String::new(),
        };

        let errors = validate_location_form(&values).unwrap_err();
        assert_eq!(errors.len(), 4);
        assert!(errors.iter().any(|e| e.contains("Title is required")));
        assert!(errors.iter().any(|e| e.contains("Address is too long")));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("Opening hours is too long"))
        );
        assert!(errors.iter().any(|e| e.contains("URL must start with")));
    }

    #[test]
    fn test_validate_location_form_trims_and_accepts_minimal_input() {
        let values = LocationFormValues {
            title: "  Legokauppa  ".to_string(),
            ..LocationFormValues::default()
        };

        let ok = validate_location_form(&values).unwrap();
        assert_eq!(ok.title, "Legokauppa");
        assert_eq!(ok.location, "");
        assert_eq!(ok.url, "");
    }

    #[test]
    fn test_location_to_payload_round_trips_through_display_yaml() {
        let loc = Location {
            uid: "abcdefghij12345".to_string(),
            title: "Museo".to_string(),
            location: Some("Museokatu 2".to_string()),
            opening_hours: Some("ti-su 11-17".to_string()),
            tags: Some(vec!["museum".to_string()]),
            point: Some(GeoPoint {
                lat: 60.1699,
                lon: 24.9384,
            }),
            ..Location::default()
        };

        let yaml = generate_location_display_yaml(&location_to_payload(&loc));
        assert!(yaml.contains("title: Museo"));
        assert!(yaml.contains("- museum"));
        assert!(yaml.contains("lat: 60.1699"));
    }
}
