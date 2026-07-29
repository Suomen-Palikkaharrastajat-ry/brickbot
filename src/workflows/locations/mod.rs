pub mod modals;
pub mod models;
pub mod pocketbase;
pub mod validation;

pub use modals::*;
pub use models::*;
pub use pocketbase::*;
pub use validation::*;

use crate::workflows::{
    AppContext, InteractionVisibility, WizardInteraction, extract_locale_and_bot_name,
};
use serenity::all::{
    ButtonStyle, Context, CreateActionRow, CreateButton, CreateInteractionResponseMessage,
};

/// Discord renders at most 25 options in a select menu.
const MAX_SELECT_OPTIONS: usize = 25;

/// Localization key for a tag's human-readable label.
#[must_use]
pub fn tag_label_key(tag: &str) -> &'static str {
    match tag {
        "exhibition" => "command.locations.tags.exhibition",
        "store" => "command.locations.tags.store",
        "fleamarket" => "command.locations.tags.fleamarket",
        "museum" => "command.locations.tags.museum",
        _ => "command.locations.tags.other",
    }
}

/// Renders the `/paikat` (`/locations`) wizard.
///
/// Mirrors the events wizard: an optional cover image is stashed as a draft, published
/// locations are offered for editing, and a tag select feeds the "propose new" modal. Unlike
/// events there is no visibility toggle — locations are not mirrored to Discord's calendar.
pub async fn handle_locations_wizard_command(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: WizardInteraction<'_>,
    status_msg: Option<String>,
    is_deferred: bool,
) -> anyhow::Result<()> {
    let guild_id_u64 = interaction
        .guild_id()
        .map(serenity::all::GuildId::get)
        .unwrap_or_default();
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id()).await;
    let cmd_cfg = app_ctx.config.commands_for(guild_id_u64).locations;

    let mut image_url = None;
    if let WizardInteraction::Command(cmd) = &interaction {
        let image_arg_name =
            rust_i18n::t!("command.locations.image_arg_name", locale = locale.as_str()).to_string();
        let mut image_attachment_id = None;
        for opt in &cmd.data.options {
            if opt.name == image_arg_name {
                if let serenity::all::CommandDataOptionValue::Attachment(att) = &opt.value {
                    image_attachment_id = Some(*att);
                }
            }
        }

        if let Some(att_id) = image_attachment_id {
            if let Some(att) = cmd.data.resolved.attachments.get(&att_id) {
                image_url = Some(att.url.clone());
            }
        }
    }

    let mut draft_id = None;
    if let Some(url) = image_url {
        let uuid = uuid::Uuid::new_v4().to_string();
        let payload = serde_json::json!({ "image_url": url });
        let _ = crate::db::insert_drafting_input(&app_ctx.db, &uuid, &payload.to_string()).await;
        draft_id = Some(uuid);
    }

    let mut components = vec![];
    let mut first_uid = None;
    let mut edit_options = vec![];
    let mut cached_locations = vec![];

    if let Some(pb_cfg) = &app_ctx.config.pocketbase {
        if let Ok(locations) = pocketbase::fetch_pocketbase_locations(
            app_ctx.http.as_ref(),
            pb_cfg,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            let mut locations = locations;
            locations.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
            // Only the options Discord will actually render are worth caching on the session.
            locations.truncate(MAX_SELECT_OPTIONS);

            if let Some(first) = locations.first() {
                first_uid = Some(first.uid.clone());
            }
            cached_locations.clone_from(&locations);

            for loc in locations {
                let addr = loc.location.unwrap_or_else(|| "—".to_string());
                let label = crate::discord_limits::truncate(
                    &format!("{} | {}", loc.title, addr),
                    crate::discord_limits::MAX_SELECT_OPTION_LABEL,
                );
                edit_options.push(serenity::builder::CreateSelectMenuOption::new(
                    label, loc.uid,
                ));
            }
        }
    }

    let mut session_id = String::new();
    if interaction.guild_id().is_some() {
        session_id = crate::db::create_workflow_session(
            &app_ctx.db,
            "main_locations_wizard",
            &interaction.user_id().get().to_string(),
            &interaction
                .guild_id()
                .map(|g| g.get().to_string())
                .unwrap_or_default(),
            &interaction.channel_id().get().to_string(),
            serde_json::json!({
                "draft_id": draft_id,
                "selected_location_uid": first_uid,
                "selected_tags": ["other"],
                "locations_cache": cached_locations
            }),
            30,
        )
        .await?;
    }

    if cmd_cfg.enable_edit && !edit_options.is_empty() {
        let edit_select = serenity::builder::CreateSelectMenu::new(
            format!("wizard_edit_location_select_change:{session_id}"),
            serenity::builder::CreateSelectMenuKind::String {
                options: edit_options,
            },
        )
        .placeholder(rust_i18n::t!(
            "command.locations.edit_prompt",
            locale = locale.as_str()
        ));
        components.push(CreateActionRow::SelectMenu(edit_select));

        components.push(CreateActionRow::Buttons(vec![
            CreateButton::new(format!("wizard_locations_edit:{session_id}"))
                .label(rust_i18n::t!(
                    "command.locations.btn_edit",
                    locale = locale.as_str()
                ))
                .style(ButtonStyle::Danger),
        ]));
    }

    if cmd_cfg.enable_propose {
        let tag_options: Vec<_> = models::LOCATION_TAGS
            .iter()
            .map(|tag| {
                let label = rust_i18n::t!(tag_label_key(tag), locale = locale.as_str());
                let option =
                    serenity::builder::CreateSelectMenuOption::new(label, (*tag).to_string());
                if *tag == "other" {
                    option.default_selection(true)
                } else {
                    option
                }
            })
            .collect();

        let tag_select = serenity::builder::CreateSelectMenu::new(
            format!("wizard_location_tag_select_change:{session_id}"),
            serenity::builder::CreateSelectMenuKind::String {
                options: tag_options,
            },
        )
        .placeholder(rust_i18n::t!(
            "command.locations.tag_prompt",
            locale = locale.as_str()
        ))
        .min_values(1)
        .max_values(models::MAX_LOCATION_TAGS);
        components.push(CreateActionRow::SelectMenu(tag_select));

        components.push(CreateActionRow::Buttons(vec![
            CreateButton::new(format!("wizard_location_submit_continue:{session_id}"))
                .label(rust_i18n::t!(
                    "command.locations.btn_submit",
                    locale = locale.as_str()
                ))
                .style(ButtonStyle::Success),
        ]));
    }

    let msg = status_msg.map_or_else(
        || rust_i18n::t!("command.locations.wizard_prompt", locale = locale.as_str()).to_string(),
        |s| format!("**{s}**"),
    );

    if is_deferred {
        let resp = serenity::builder::EditInteractionResponse::new()
            .content(msg)
            .components(components);
        interaction.edit_response(ctx, resp).await?;
    } else {
        let resp = CreateInteractionResponseMessage::new()
            .content(msg)
            .components(components);
        interaction
            .create_or_update_response(ctx, resp, InteractionVisibility::Ephemeral)
            .await?;
    }

    Ok(())
}
