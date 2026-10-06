//! Part-picker and the build-thread buttons.

use super::{
    ChannelId, ComponentInteraction, Config, Context, CreateInteractionResponse, CreateMessage,
    build_cmd, db, dispatch, library_for, max_size_for, parts_cache_for, persist, post_export,
    post_render, session_lock,
};

/// Offers the `/part` search results for a part the language could not
/// resolve.
pub(super) async fn offer_part_choices(
    ctx: &Context,
    config: &Config,
    channel_id: ChannelId,
    pending: &dispatch::PendingLookup,
    session_id: &str,
    locale: &str,
) -> anyhow::Result<()> {
    let http = crate::http::HttpClient::new();
    // Read through the sticky cache, so a disambiguation in a build thread
    // warms the same index part autocomplete reads.
    let cache = parts_cache_for(ctx).await;
    let response = crate::commands::part::part_interaction(
        ctx,
        &http,
        cache.as_deref(),
        &pending.query,
        locale,
        &["bricklink".to_string(), "rebrickable".to_string()],
        config.resource_limits.max_http_body_bytes,
    )
    .await?;

    if let crate::commands::part::PartInteractionResponse::SearchResults(options) = response {
        let menu = serenity::all::CreateSelectMenu::new(
            // The session id rides along, so the router can find the session
            // and the handler can check the choice still belongs to it.
            format!("{}:{session_id}", build_cmd::SELECT_PART),
            serenity::all::CreateSelectMenuKind::String { options },
        )
        .placeholder(rust_i18n::t!("command.part.search_placeholder", locale = locale).to_string());

        channel_id
            .send_message(
                &ctx.http,
                CreateMessage::new()
                    .content(
                        rust_i18n::t!("command.part.search_results_content", locale = locale)
                            .to_string(),
                    )
                    .components(vec![serenity::all::CreateActionRow::SelectMenu(menu)]),
            )
            .await?;
    }
    Ok(())
}

/// Handles a choice from the `build_part_pick` dropdown.
///
/// Finishes the sentence that paused on an unknown part by replaying it with
/// the chosen element. Without this the menu was posted but nothing listened,
/// so picking an option just showed "This interaction failed".
pub async fn handle_part_choice(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let cfg = app_ctx.config.commands_for(guild_id).build;
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();

    let serenity::all::ComponentInteractionDataKind::StringSelect { values } =
        &interaction.data.kind
    else {
        return Ok(());
    };
    let Some(part_num) = values.first() else {
        return Ok(());
    };

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return Ok(());
    };
    if session.id != session_id {
        return Ok(());
    }

    // A choice is only meaningful against the sentence it was offered for. If
    // that has been cleared — because the model has moved on since — say so
    // rather than replaying something stale.
    let Some(sentence) = session.pending_sentence.clone() else {
        let _ = interaction
            .channel_id
            .say(
                &ctx.http,
                rust_i18n::t!("command.build.pick_expired", locale = locale.as_str()).to_string(),
            )
            .await;
        return Ok(());
    };
    // What the paused sentence actually asked for, e.g. `Add a plate...`,
    // so the chosen element keeps that shape's height rather than always
    // becoming a brick. `kind_from_name` still gets the last word if the
    // Rebrickable result's own name disagrees.
    let asked_kind = session
        .pending_kind
        .unwrap_or(crate::build::model::PartKind::Brick);

    let http = crate::http::HttpClient::new();
    let part = crate::parts_cache::warm::detail(
        app_ctx.parts_cache.as_deref(),
        &http,
        part_num,
        app_ctx.config.resource_limits.max_http_body_bytes,
    )
    .await?;
    let chosen = crate::build::parts::from_rebrickable(
        &part.part_num,
        &part.name,
        &part.external_ids,
        asked_kind,
    );

    let lock = session_lock(&session.id).await;
    let guard = lock.lock().await;

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let builders = dispatch::Builders::new([(
        interaction.user.id.get().to_string(),
        interaction.user.id.get().to_string(),
    )]);
    let author = interaction.user.id.get().to_string();
    let outcome = dispatch::process_message_with_part(
        &history,
        &sentence,
        &locale,
        &builders,
        &author,
        &chosen,
        max_size_for(&session, &cfg),
    );
    persist(&app_ctx.db, &session.id, &history, &outcome, &author).await?;
    drop(guard);

    for reply in &outcome.replies {
        let _ = interaction.channel_id.say(&ctx.http, reply).await;
    }

    if outcome.needs_render {
        let history =
            db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
        let library = library_for(ctx).await;
        post_render(
            ctx,
            interaction.channel_id,
            &history,
            &library,
            &cfg,
            &session.id,
            &locale,
        )
        .await;
    }

    Ok(())
}

/// Handles the `[Export .io]`, `[Undo]` and `[End session]` buttons.
pub async fn handle_build_button(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    base_id: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let cfg = app_ctx.config.commands_for(guild_id).build;
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();

    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return Ok(());
    };
    if session.id != session_id {
        return Ok(());
    }

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
        .await?;

    let mut history =
        db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let library = library_for(ctx).await;

    match base_id {
        build_cmd::BUTTON_EXPORT => {
            post_export(
                ctx,
                interaction.channel_id,
                &history,
                &library,
                &cfg,
                &locale,
            )
            .await;
        }
        build_cmd::BUTTON_UNDO => {
            if history.undo() {
                db::build::set_cursor(&app_ctx.db, &session.id, history.revision()).await?;
                post_render(
                    ctx,
                    interaction.channel_id,
                    &history,
                    &library,
                    &cfg,
                    &session.id,
                    &locale,
                )
                .await;
            } else {
                let _ = interaction
                    .channel_id
                    .say(
                        &ctx.http,
                        rust_i18n::t!("command.build.nothing_to_undo", locale = locale.as_str())
                            .to_string(),
                    )
                    .await;
            }
        }
        build_cmd::BUTTON_FINISH => {
            db::build::close_session(&app_ctx.db, &session.id).await?;
            let model = history.materialize();
            let _ = interaction
                .channel_id
                .say(&ctx.http, build_cmd::finished_message(&model, &locale))
                .await;
        }
        _ => {}
    }

    Ok(())
}

// --- Private drawing drafts ------------------------------------------------

// --- Placement proposals ---------------------------------------------------
