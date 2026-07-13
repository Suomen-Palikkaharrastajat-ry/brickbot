pub mod modals;
pub mod models;
pub mod outbox;

pub mod validation;
pub mod visibility;

pub use modals::*;
pub use models::*;
pub use outbox::*;
pub use validation::*;
pub use visibility::*;

use crate::workflows::{AppContext, extract_locale_and_bot_name};
use serenity::all::{
    ButtonStyle, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateInteractionResponseMessage,
};

#[allow(dead_code)]
pub enum InteractionVisibility {
    Ephemeral,
    Public,
}

#[allow(dead_code)]
pub enum WizardInteraction<'a> {
    Command(&'a CommandInteraction),
    Component(&'a ComponentInteraction),
}

#[allow(dead_code)]
impl WizardInteraction<'_> {
    #[allow(dead_code)]
    pub async fn edit_response(
        &self,
        ctx: &Context,
        resp: serenity::builder::EditInteractionResponse,
    ) -> serenity::Result<serenity::all::Message> {
        match self {
            WizardInteraction::Command(c) => c.edit_response(&ctx.http, resp).await,
            WizardInteraction::Component(c) => c.edit_response(&ctx.http, resp).await,
        }
    }

    pub async fn create_or_update_response(
        &self,
        ctx: &Context,
        mut resp: CreateInteractionResponseMessage,
        visibility: InteractionVisibility,
    ) -> serenity::Result<()> {
        let ephemeral = matches!(visibility, InteractionVisibility::Ephemeral);
        resp = resp.ephemeral(ephemeral);
        match self {
            WizardInteraction::Command(c) => {
                c.create_response(&ctx.http, CreateInteractionResponse::Message(resp))
                    .await
            }
            WizardInteraction::Component(c) => {
                c.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(resp))
                    .await
            }
        }
    }

    pub const fn guild_id(&self) -> Option<serenity::all::GuildId> {
        match self {
            WizardInteraction::Command(c) => c.guild_id,
            WizardInteraction::Component(c) => c.guild_id,
        }
    }
}

pub async fn handle_events_wizard_command(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: WizardInteraction<'_>,
    status_msg: Option<String>,
    is_deferred: bool,
) -> anyhow::Result<()> {
    let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id()).await;

    let mut image_url = None;
    if let WizardInteraction::Command(cmd) = &interaction {
        let mut image_attachment_id = None;
        for opt in &cmd.data.options {
            if opt.name == "image" {
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
        let payload = serde_json::json!({
            "image_url": url
        });
        let _ = crate::db::insert_drafting_input(&app_ctx.db, &uuid, &payload.to_string()).await;
        draft_id = Some(uuid);
    }

    let mut components = vec![];
    let mut first_uid = None;
    let mut edit_options = vec![];

    if let Some(pb_cfg) = &app_ctx.config.pocketbase {
        if let Ok(events) = crate::events_sync::fetch_pocketbase_events(
            app_ctx.http.as_ref(),
            pb_cfg,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            let now = chrono::Utc::now();
            let mut upcoming_events: Vec<_> = events
                .into_iter()
                .filter(|e| e.end_time.unwrap_or(e.start_time) >= now)
                .collect();
            upcoming_events.sort_by_key(|e| e.start_time);

            if let Some(first) = upcoming_events.first() {
                first_uid = Some(first.uid.clone());
            }

            let guild_id_str = interaction
                .guild_id()
                .map(|g| g.get().to_string())
                .unwrap_or_default();
            let hidden_events = crate::db::get_hidden_events(&app_ctx.db, &guild_id_str)
                .await
                .unwrap_or_default();
            let hidden_set: std::collections::HashSet<String> = hidden_events.into_iter().collect();

            for e in upcoming_events.into_iter().take(25) {
                let date_str = e.start_time.format("%d.%m.%Y").to_string();
                let loc_str = e.location.clone().unwrap_or_else(|| "TBD".to_string());
                let label = crate::discord_limits::truncate(
                    &format!("{} | {} | {}", date_str, e.summary, loc_str),
                    crate::discord_limits::MAX_SELECT_OPTION_LABEL,
                );

                let is_visible = !hidden_set.contains(&e.uid);
                let display_label = if is_visible {
                    crate::discord_limits::truncate(
                        &format!("✅ {label}"),
                        crate::discord_limits::MAX_SELECT_OPTION_LABEL,
                    )
                } else {
                    crate::discord_limits::truncate(
                        &format!("❌ {label}"),
                        crate::discord_limits::MAX_SELECT_OPTION_LABEL,
                    )
                };

                edit_options.push(serenity::builder::CreateSelectMenuOption::new(
                    display_label,
                    e.uid,
                ));
            }
        }
    }

    let mut session_id = String::new();
    if let Some(sid) = interaction.guild_id() {
        let user_id = match &interaction {
            WizardInteraction::Command(c) => c.user.id.get().to_string(),
            WizardInteraction::Component(c) => c.user.id.get().to_string(),
        };
        let guild_id = sid.get().to_string();
        let channel_id = match &interaction {
            WizardInteraction::Command(c) => c.channel_id.get().to_string(),
            WizardInteraction::Component(c) => c.channel_id.get().to_string(),
        };
        session_id = crate::db::create_workflow_session(
            &app_ctx.db,
            "main_events_wizard",
            &user_id,
            &guild_id,
            &channel_id,
            serde_json::json!({
                "draft_id": draft_id,
                "selected_event_uid": first_uid,
                "selected_tag": "event"
            }),
            30,
        )
        .await?;
    }

    let mut action_buttons_top = vec![];
    if app_ctx.config.commands.events.enable_edit && !edit_options.is_empty() {
        let edit_select = serenity::builder::CreateSelectMenu::new(
            format!("wizard_edit_event_select_change:{session_id}"),
            serenity::builder::CreateSelectMenuKind::String {
                options: edit_options,
            },
        )
        .placeholder(rust_i18n::t!(
            "command.events.edit_prompt",
            locale = locale.as_str()
        ));
        components.push(CreateActionRow::SelectMenu(edit_select));

        action_buttons_top.push(
            CreateButton::new(format!("wizard_events_edit:{session_id}"))
                .label(rust_i18n::t!(
                    "command.events.btn_edit",
                    locale = locale.as_str()
                ))
                .style(ButtonStyle::Danger),
        );
    }

    action_buttons_top.push(
        CreateButton::new(format!("wizard_events_toggle_visibility:{session_id}"))
            .label(rust_i18n::t!(
                "command.events.choice_visibility",
                locale = locale.as_str()
            ))
            .style(ButtonStyle::Secondary),
    );

    if !action_buttons_top.is_empty() {
        components.push(CreateActionRow::Buttons(action_buttons_top));
    }

    let mut action_buttons_bottom = vec![];

    if app_ctx.config.commands.events.enable_propose {
        let ex_lbl = rust_i18n::t!("command.events.tags.exhibition", locale = locale.as_str());
        let ev_lbl = rust_i18n::t!("command.events.tags.event", locale = locale.as_str());
        let co_lbl = rust_i18n::t!("command.events.tags.competition", locale = locale.as_str());
        let sel_options = vec![
            serenity::builder::CreateSelectMenuOption::new(ex_lbl, "exhibition"),
            serenity::builder::CreateSelectMenuOption::new(ev_lbl, "event").default_selection(true),
            serenity::builder::CreateSelectMenuOption::new(co_lbl, "competition"),
        ];
        let type_select = serenity::builder::CreateSelectMenu::new(
            format!("wizard_submit_tag_select_change:{session_id}"),
            serenity::builder::CreateSelectMenuKind::String {
                options: sel_options,
            },
        )
        .placeholder(rust_i18n::t!(
            "command.events.tag_prompt",
            locale = locale.as_str()
        ));
        components.push(CreateActionRow::SelectMenu(type_select));
    }

    if app_ctx.config.commands.events.enable_propose {
        action_buttons_bottom.push(
            CreateButton::new(format!("wizard_submit_tag_continue:{session_id}"))
                .label(rust_i18n::t!(
                    "command.events.btn_submit",
                    locale = locale.as_str()
                ))
                .style(ButtonStyle::Success),
        );
    }

    if !action_buttons_bottom.is_empty() {
        components.push(CreateActionRow::Buttons(action_buttons_bottom));
    }

    let msg = status_msg.map_or_else(
        || rust_i18n::t!("command.events.wizard_prompt", locale = locale.as_str()).to_string(),
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
