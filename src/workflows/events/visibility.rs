use crate::workflows::events::{InteractionVisibility, WizardInteraction};
use crate::workflows::{AppContext, extract_locale_and_bot_name};
use serenity::all::{
    ComponentInteraction, Context, CreateActionRow, CreateInteractionResponseMessage,
};

pub async fn handle_wizard_events_toggle_visibility(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    let user_id = interaction.user.id.get().to_string();
    let payload =
        crate::db::get_workflow_session_payload(&app_ctx.db, session_id, &user_id).await?;

    let selected_event_uid = payload
        .as_ref()
        .and_then(|p| p.get("selected_event_uid"))
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string);

    let Some(selected_uid) = selected_event_uid else {
        return Ok(());
    };

    let Some(guild_id) = interaction.guild_id else {
        return Ok(());
    };

    let (locale, _) =
        crate::workflows::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let updating_msg = rust_i18n::t!(
        "command.events.updating_visibility",
        locale = locale.as_str()
    );

    let mut resp = serenity::builder::CreateInteractionResponseMessage::new().content(updating_msg);
    resp = resp.components(vec![]);

    let _ = interaction
        .create_response(
            &ctx.http,
            serenity::builder::CreateInteractionResponse::UpdateMessage(resp),
        )
        .await;

    if let Some(pb_cfg) = &app_ctx.config.pocketbase {
        let _guard = app_ctx.sync_mutex.lock().await;
        let hidden_events = crate::db::get_hidden_events(&app_ctx.db, &guild_id.to_string())
            .await
            .unwrap_or_default();
        let hidden_set: std::collections::HashSet<String> = hidden_events.into_iter().collect();

        if hidden_set.contains(&selected_uid) {
            let _ =
                crate::db::unhide_event(&app_ctx.db, &guild_id.to_string(), &selected_uid).await;
        } else {
            let _ = crate::db::hide_event(&app_ctx.db, &guild_id.to_string(), &selected_uid).await;
        }

        let events = crate::events_sync::fetch_pocketbase_events(
            app_ctx.http.as_ref(),
            pb_cfg,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        .unwrap_or_default();

        let event_name = events
            .iter()
            .find(|e| e.uid == selected_uid)
            .map_or_else(|| selected_uid.clone(), |e| e.summary.clone());

        let _ = crate::events_sync::sync_events_with_discord(
            &ctx.http,
            app_ctx.http.as_ref(),
            &app_ctx.db,
            guild_id,
            events,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await;

        let (locale, _) = extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

        let msg = if hidden_set.contains(&selected_uid) {
            rust_i18n::t!(
                "command.events.visibility_restored",
                locale = locale.as_str(),
                name = event_name
            )
            .to_string()
        } else {
            rust_i18n::t!(
                "command.events.visibility_hidden",
                locale = locale.as_str(),
                name = event_name
            )
            .to_string()
        };

        let resp = serenity::builder::EditInteractionResponse::new()
            .content(msg)
            .components(vec![]);
        let _ = interaction.edit_response(&ctx.http, resp).await;
    } else {
        // Fallback if pocketbase is not enabled
        let resp = serenity::builder::EditInteractionResponse::new()
            .content(rust_i18n::t!(
                "errors.pocketbase_disabled",
                locale = locale.as_str()
            ))
            .components(vec![]);
        let _ = interaction.edit_response(&ctx.http, resp).await;
    }

    Ok(())
}
