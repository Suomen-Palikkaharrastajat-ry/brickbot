//! Thread creation and chat messages in a build thread.

use super::{
    BuildDebugLog, Classification, CommandInteraction, Config, Context, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateThread, MessageContext, Outcome,
    ProposalContext, ReactionType, build_cmd, builders_of, cached_is_thread, cancel_pending_render,
    db, dispatch, is_definitely_not_a_thread, library_for, max_size_for, offer_part_choices,
    offer_proposals, persist, post_export, post_render, respond_ephemeral, schedule_render,
    session_lock,
};

/// Handles `/build <name>`: announce, open a thread, register the session.
pub async fn handle_build_command(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    command: &CommandInteraction,
) -> anyhow::Result<()> {
    let (locale, _) =
        crate::workflows::extract_locale_and_bot_name(app_ctx, command.guild_id).await;
    let cfg = app_ctx
        .config
        .commands_for(command.guild_id.unwrap_or_default().get())
        .build;

    let model_name = command
        .data
        .options
        .iter()
        .find(|opt| build_cmd::option_name_matches(&opt.name))
        .and_then(|opt| opt.value.as_str())
        .unwrap_or("Model")
        .to_owned();

    // `size` is free-typed rather than a set of choices, so it can be
    // anything — validate it before touching Discord or the database, the
    // same way an unparseable channel would be refused before creating one.
    let size_arg = command
        .data
        .options
        .iter()
        .find(|opt| build_cmd::size_option_name_matches(&opt.name))
        .and_then(|opt| opt.value.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let max_size = match size_arg.map(crate::build::units::MaxModelSize::parse) {
        None => None,
        Some(Some(parsed)) => Some(parsed),
        Some(None) => {
            return respond_ephemeral(
                ctx,
                command,
                &rust_i18n::t!(
                    "command.build.invalid_size",
                    locale = locale.as_str(),
                    size = size_arg.unwrap_or_default()
                ),
            )
            .await;
        }
    };

    // Discord does not allow nesting threads, so refuse early with a useful
    // message rather than failing at creation time. An uncached channel is
    // assumed not to be a thread: the worst case is the friendlier error being
    // replaced by Discord's own.
    if cached_is_thread(ctx, command.channel_id) == Some(true) {
        return respond_ephemeral(
            ctx,
            command,
            &rust_i18n::t!("command.build.already_in_thread", locale = locale.as_str()),
        )
        .await;
    }

    // Respond publicly: a build session is deliberately visible, unlike the
    // ambient suggestions ADR-001's ephemeral policy is about.
    command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .add_embed(build_cmd::build_intro_embed(&model_name, &locale)),
            ),
        )
        .await?;

    let anchor = command.get_response(&ctx.http).await?;
    let thread = command
        .channel_id
        .create_thread_from_message(
            &ctx.http,
            anchor.id,
            CreateThread::new(build_cmd::thread_name(&model_name, &locale))
                .auto_archive_duration(serenity::all::AutoArchiveDuration::OneDay),
        )
        .await?;

    let session = db::build::create_build_session(
        &app_ctx.db,
        &command.guild_id.unwrap_or_default().get().to_string(),
        &thread.id.get().to_string(),
        &command.user.id.get().to_string(),
        &model_name,
        cfg.session_ttl_hours,
        max_size,
    )
    .await?;

    // Say once, up front, if renders will be approximate.
    let library = library_for(ctx).await;
    if !library.is_available() {
        let _ = thread
            .id
            .send_message(
                &ctx.http,
                CreateMessage::new().content(
                    rust_i18n::t!("command.build.box_mode_notice", locale = locale.as_str())
                        .to_string(),
                ),
            )
            .await;
    }

    // The starter message carries the same button row every picture carries,
    // so [Draw…] exists before anything has been built.
    let _ = thread
        .id
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .content(build_cmd::thread_intro_message(&model_name, &locale))
                .components(vec![build_cmd::build_session_buttons(
                    &session.id,
                    &locale,
                    cfg.enable_io_export,
                )]),
        )
        .await;

    tracing::info!(
        "build: opened session {} in thread {} for model '{}'",
        session.id,
        thread.id,
        model_name
    );
    Ok(())
}

/// Handles one message in a thread.
///
/// Returns `Ok(true)` when the message belonged to a build session and has
/// been dealt with, so the caller must not fall through to the ambient
/// assistant.
pub async fn handle_thread_message(
    ctx: &Context,
    pool: &sqlx::SqlitePool,
    config: &Config,
    msg: &serenity::all::Message,
    debug_log: Option<&BuildDebugLog>,
) -> anyhow::Result<bool> {
    // Cheapest possible rejection first. A channel the cache knows is not a
    // thread cannot hold a session; anything else falls through to the indexed
    // lookup below, so a cold cache after a restart does not silently swallow
    // a live build thread.
    if is_definitely_not_a_thread(ctx, msg.channel_id) {
        return Ok(false);
    }

    let channel_id = msg.channel_id.get().to_string();
    let Some(session) = db::build::get_active_session_by_channel(pool, &channel_id).await? else {
        return Ok(false);
    };

    let guild_id = msg.guild_id.unwrap_or_default().get();
    let cfg = config.commands_for(guild_id).build;
    let locale = config.locale_or_default(guild_id).to_string();
    let author = msg.author.id.get().to_string();

    // Most messages in a build thread are conversation, and parsing needs no
    // model, so rule those out before taking the session lock and loading the
    // history behind it. The message still belonged to the session.
    if dispatch::is_ordinary_chat(&msg.content) {
        log_thread_message(
            debug_log,
            &session.id,
            msg,
            &author,
            &locale,
            Classification::Ignored,
            None,
        )
        .await;
        return Ok(true);
    }

    // Everything from loading the history to writing it back must be atomic
    // with respect to other messages in this session, or simultaneous builders
    // silently overwrite each other.
    let lock = session_lock(&session.id).await;
    let guard = lock.lock().await;

    let history = db::build::load_history(pool, &session.id, &session.model_name).await?;
    let builders = builders_of(ctx, msg, &history);
    let outcome = dispatch::process_message(
        &history,
        &msg.content,
        &locale,
        &builders,
        &author,
        max_size_for(&session, &cfg),
    );

    // Ordinary conversation: say nothing at all.
    if outcome.is_silent() {
        log_thread_message(
            debug_log,
            &session.id,
            msg,
            &author,
            &locale,
            Classification::Silent,
            Some(&outcome),
        )
        .await;
        return Ok(true);
    }

    log_thread_message(
        debug_log,
        &session.id,
        msg,
        &author,
        &locale,
        Classification::of(&outcome),
        Some(&outcome),
    )
    .await;

    persist(pool, &session.id, &history, &outcome, &author).await?;
    let _ = db::build::touch_session(pool, &session.id, cfg.session_ttl_hours).await;
    // The write is done; the Discord calls below do not need the lock and are
    // far slower than it, so let the next message in.
    drop(guard);

    for reaction in &outcome.reactions {
        let _ = msg
            .react(&ctx.http, ReactionType::Unicode((*reaction).to_string()))
            .await;
    }
    for reply in &outcome.replies {
        // Replies can carry a `<@id>` from "Who added the roof?", which should
        // read as a name rather than ping the person mid-build.
        let _ = msg
            .channel_id
            .send_message(
                &ctx.http,
                CreateMessage::new()
                    .content(reply)
                    .allowed_mentions(serenity::all::CreateAllowedMentions::new()),
            )
            .await;
    }

    if let Some(pending) = &outcome.pending_lookup {
        // Hand the ambiguity to the existing /part search UI.
        let _ =
            offer_part_choices(ctx, config, msg.channel_id, pending, &session.id, &locale).await;
    }

    if let Some(pending) = &outcome.pending_proposal {
        // Something was named without saying where it goes: offer the places
        // it could legally land and let whoever asked pick.
        let _ = offer_proposals(
            ctx,
            &ProposalContext {
                pool,
                session: &session,
                user_id: &msg.author.id.get().to_string(),
                guild_id: msg.guild_id.unwrap_or_default().get(),
                channel_id: msg.channel_id,
                cfg: &cfg,
                locale: &locale,
            },
            pending,
        )
        .await;
    }

    if outcome.needs_render {
        // Debounced: four people building at once should produce one picture,
        // not four. An export or the end of a session is a deliberate act, so
        // those below are immediate.
        schedule_render(ctx, pool, &session, &cfg, &locale, msg.channel_id).await;
    }

    if outcome.needs_export && cfg.enable_io_export {
        let history = db::build::load_history(pool, &session.id, &session.model_name).await?;
        let library = library_for(ctx).await;
        post_export(ctx, msg.channel_id, &history, &library, &cfg, &locale).await;
    }

    if outcome.finished {
        // A pending render would otherwise land after the closing message.
        cancel_pending_render(&session.id).await;
        db::build::close_session(pool, &session.id).await?;
        let history = db::build::load_history(pool, &session.id, &session.model_name).await?;
        let library = library_for(ctx).await;
        post_render(
            ctx,
            msg.channel_id,
            &history,
            &library,
            &cfg,
            &session.id,
            &locale,
        )
        .await;
        let _ = msg
            .channel_id
            .say(
                &ctx.http,
                build_cmd::finished_message(&history.materialize(), &locale),
            )
            .await;
    }

    Ok(true)
}

/// Appends to the debug log when one is configured; a no-op otherwise.
pub(super) async fn log_thread_message(
    debug_log: Option<&BuildDebugLog>,
    session_id: &str,
    msg: &serenity::all::Message,
    author: &str,
    locale: &str,
    classification: Classification,
    outcome: Option<&Outcome>,
) {
    if let Some(log) = debug_log {
        log.record(
            MessageContext {
                session_id,
                channel_id: msg.channel_id.get(),
                author_id: author,
                locale,
                text: &msg.content,
            },
            classification,
            outcome,
        )
        .await;
    }
}
