#![allow(
    clippy::significant_drop_tightening,
    clippy::useless_let_if_seq,
    clippy::needless_pass_by_value,
    clippy::format_push_string,
    unused_imports,
    clippy::similar_names,
    clippy::too_many_lines
)]
pub mod ambient;
pub mod events;
pub mod search;

use serenity::all::{CommandInteraction, ComponentInteraction, Context, ModalInteraction};

pub struct AppContext {
    pub db: sqlx::SqlitePool,
    pub config: std::sync::Arc<crate::config::Config>,
    pub http: std::sync::Arc<dyn crate::http::HttpProvider>,
    pub sync_mutex: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl AppContext {
    pub async fn from_serenity_ctx(ctx: &Context) -> Self {
        let data = ctx.data.read().await;
        let config = data
            .get::<crate::ConfigData>()
            .expect("Config missing")
            .clone();
        let db = data.get::<crate::DbData>().expect("DbData missing").clone();
        let http = std::sync::Arc::new(crate::http::HttpClient::new());
        let sync_mutex = data
            .get::<crate::SyncMutexData>()
            .expect("SyncMutexData missing")
            .clone();
        Self {
            db,
            config,
            http,
            sync_mutex,
        }
    }
}

pub async fn extract_locale_and_bot_name(
    app_ctx: &AppContext,
    guild_id: Option<serenity::all::GuildId>,
) -> (String, String) {
    let gid = guild_id.unwrap_or_default().get();
    let guild_conf = app_ctx.config.guilds.iter().find(|g| g.guild_id == gid);

    let locale = guild_conf
        .and_then(|g| g.locale.clone())
        .or_else(|| app_ctx.config.default_locale.clone())
        .unwrap_or_else(|| "en-US".to_string());

    let name = guild_conf
        .and_then(|g| g.bot_name.clone())
        .or_else(|| app_ctx.config.bot_name.clone())
        .unwrap_or_else(|| rust_i18n::t!("common.default_bot_name", locale = &locale).to_string());

    (locale, name)
}

pub async fn handle_component_interaction(
    ctx: &Context,
    interaction: &ComponentInteraction,
) -> anyhow::Result<()> {
    let app_ctx = AppContext::from_serenity_ctx(ctx).await;
    let custom_id = &interaction.data.custom_id;
    let mut base_id = custom_id.as_str();
    let mut arg = None;
    if let Some((b, a)) = custom_id.split_once(':') {
        base_id = b;
        arg = Some(a);
    }

    let (locale, _) = extract_locale_and_bot_name(&app_ctx, interaction.guild_id).await;

    match base_id {
        "workflow_ignore" => {
            ambient::handle_workflow_ignore(ctx, &app_ctx, interaction, arg, &locale).await
        }
        "workflow_ignore_always" => {
            ambient::handle_workflow_ignore_always(ctx, &app_ctx, interaction, &locale).await
        }
        "update_services_set" => {
            search::handle_update_services_set(ctx, &app_ctx, interaction, arg, &locale).await
        }
        "workflow_set_search" => {
            search::handle_workflow_set_search(ctx, &app_ctx, interaction, arg, &locale).await
        }
        "workflow_part_search" => {
            search::handle_workflow_part_search(ctx, &app_ctx, interaction, arg, &locale).await
        }
        "wizard_events_toggle_visibility" => {
            events::handle_wizard_events_toggle_visibility(
                ctx,
                &app_ctx,
                interaction,
                arg.unwrap_or_default(),
            )
            .await
        }
        "wizard_events_edit" => {
            events::handle_wizard_events_edit(ctx, &app_ctx, interaction, arg).await
        }
        "wizard_edit_event_select_change" => {
            events::handle_wizard_edit_event_select_change(
                ctx,
                &app_ctx,
                interaction,
                arg.unwrap_or_default(),
            )
            .await
        }
        "wizard_submit_tag_select_change" => {
            events::handle_wizard_submit_tag_select_change(
                ctx,
                &app_ctx,
                interaction,
                arg.unwrap_or_default(),
            )
            .await
        }
        "wizard_submit_tag_continue" => {
            events::handle_wizard_submit_tag_continue(ctx, &app_ctx, interaction, arg).await
        }
        "wizard_retry_submit" => {
            events::handle_wizard_retry_submit(ctx, &app_ctx, interaction, arg).await
        }
        _ => Ok(()),
    }
}

pub async fn handle_modal_submit(
    ctx: &Context,
    interaction: &ModalInteraction,
) -> anyhow::Result<()> {
    let app_ctx = AppContext::from_serenity_ctx(ctx).await;
    let custom_id = interaction.data.custom_id.as_str();
    let (locale, _) = extract_locale_and_bot_name(&app_ctx, interaction.guild_id).await;

    if custom_id.starts_with("modal_set_search") {
        search::handle_modal_set_search(ctx, &app_ctx, interaction, &locale).await?;
    } else if custom_id.starts_with("modal_part_search") {
        search::handle_modal_part_search(ctx, &app_ctx, interaction, &locale).await?;
    } else if custom_id.starts_with("modal_submit_event") {
        events::handle_modal_submit_event(ctx, &app_ctx, interaction).await?;
    } else if let Some(uid) = custom_id.strip_prefix("modal_edit_event_") {
        events::handle_modal_edit_event(ctx, &app_ctx, interaction, uid).await?;
    }

    Ok(())
}

pub async fn handle_events_wizard_command(
    ctx: &Context,
    interaction: &CommandInteraction,
) -> anyhow::Result<()> {
    let app_ctx = AppContext::from_serenity_ctx(ctx).await;
    events::handle_events_wizard_command(
        ctx,
        &app_ctx,
        events::WizardInteraction::Command(interaction),
        None,
        false,
    )
    .await
}

pub use ambient::{generate_suggestion_components, generate_suggestion_text};
