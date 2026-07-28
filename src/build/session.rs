//! The Discord side of a build session.
//!
//! The only module in [`super`] that touches serenity or sqlx. It is
//! deliberately thin: every decision has already been made by
//! [`super::dispatch`] or [`super::report`], which is where the tests are.
//! What is left here is thread creation, persistence, and the render loop.

#![allow(clippy::doc_markdown, clippy::too_many_lines)]

use super::dispatch::{self, Outcome};
use super::ops::History;
use super::scene::{self, SceneMode};
use crate::commands::build as build_cmd;
use crate::config::{BuildCommandConfig, Config};
use crate::db;
use crate::ldraw::resolve::LdrawLibrary;
use crate::render::{RenderOptions, SceneTriangle};
use serenity::all::{
    ChannelId, ChannelType, CommandInteraction, ComponentInteraction, Context, CreateAttachment,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateThread,
    ReactionType,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// How many renders may run at once, process-wide.
///
/// Rendering is CPU-bound and runs on a blocking thread; without a cap, a few
/// busy threads would starve the gateway.
static RENDER_SLOTS: std::sync::OnceLock<Semaphore> = std::sync::OnceLock::new();

fn render_slots() -> &'static Semaphore {
    RENDER_SLOTS.get_or_init(|| Semaphore::new(2))
}

/// Decides whether a render should happen now or be left to a later message.
///
/// Pure, so the debounce policy is testable without a clock or a gateway.
#[must_use]
pub const fn should_render_now(since_last_ms: u64, debounce_ms: u64) -> bool {
    since_last_ms >= debounce_ms
}

/// Whether a message could possibly belong to a build thread.
///
/// This runs for every message in every channel of every guild, so it must be
/// nearly free. A channel-kind check off the in-memory cache rejects
/// essentially all traffic before any database work.
fn is_thread(ctx: &Context, channel_id: ChannelId) -> bool {
    ctx.cache.channel(channel_id).is_some_and(|channel| {
        matches!(
            channel.kind,
            ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
        )
    })
}

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

    // Discord does not allow nesting threads, so refuse early with a useful
    // message rather than failing at creation time.
    if is_thread(ctx, command.channel_id) {
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
            CreateThread::new(build_cmd::thread_name(&model_name))
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
    )
    .await?;

    // Say once, up front, if renders will be approximate.
    let library = library_for(ctx, &app_ctx.config).await;
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
) -> anyhow::Result<bool> {
    // Cheapest possible rejection first.
    if !is_thread(ctx, msg.channel_id) {
        return Ok(false);
    }

    let channel_id = msg.channel_id.get().to_string();
    let Some(session) = db::build::get_active_session_by_channel(pool, &channel_id).await? else {
        return Ok(false);
    };

    let guild_id = msg.guild_id.unwrap_or_default().get();
    let cfg = config.commands_for(guild_id).build;
    let locale = config.locale_for(guild_id).unwrap_or("en-US").to_string();

    let history = db::build::load_history(pool, &session.id, &session.model_name).await?;
    let author = msg.author.id.get().to_string();
    let outcome = dispatch::process_message(&history, &msg.content, &locale);

    // Ordinary conversation: say nothing at all.
    if outcome.is_silent() {
        return Ok(true);
    }

    persist(pool, &session.id, &history, &outcome, &author).await?;
    let _ = db::build::touch_session(pool, &session.id, cfg.session_ttl_hours).await;

    for reaction in &outcome.reactions {
        let _ = msg
            .react(&ctx.http, ReactionType::Unicode((*reaction).to_string()))
            .await;
    }
    for reply in &outcome.replies {
        let _ = msg.channel_id.say(&ctx.http, reply).await;
    }

    if let Some(pending) = &outcome.pending_lookup {
        // Hand the ambiguity to the existing /part search UI.
        let _ = offer_part_choices(ctx, config, msg.channel_id, pending, &locale).await;
    }

    if outcome.needs_render || outcome.needs_export {
        let history = db::build::load_history(pool, &session.id, &session.model_name).await?;
        let model = history.materialize();
        let library = library_for(ctx, config).await;

        if outcome.needs_render {
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
        }
        if outcome.needs_export && cfg.enable_io_export {
            post_export(ctx, msg.channel_id, &history, &library, &cfg, &locale).await;
        }

        if outcome.finished {
            db::build::close_session(pool, &session.id).await?;
            let _ = msg
                .channel_id
                .say(&ctx.http, build_cmd::finished_message(&model, &locale))
                .await;
        }
    } else if outcome.finished {
        db::build::close_session(pool, &session.id).await?;
        let model = history.materialize();
        let _ = msg
            .channel_id
            .say(&ctx.http, build_cmd::finished_message(&model, &locale))
            .await;
    }

    Ok(true)
}

/// Writes an outcome's operations and cursor movement to the database.
async fn persist(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    history: &History,
    outcome: &Outcome,
    author: &str,
) -> anyhow::Result<()> {
    if !outcome.ops.is_empty() {
        // A new command after an undo discards the redo tail, exactly as any
        // editor does.
        #[allow(clippy::cast_possible_truncation)]
        let start = history.cursor as u32;
        db::build::truncate_from(pool, session_id, start).await?;
        for (offset, op) in outcome.ops.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let n = start + offset as u32;
            db::build::append_revision(pool, session_id, n, author, op).await?;
        }
    }

    if let Some(cursor) = outcome.cursor {
        db::build::set_cursor(pool, session_id, cursor).await?;
    }

    if let Some(author) = &outcome.skip_last_by {
        if let Some(index) = history.ops[..history.cursor]
            .iter()
            .rposition(|entry| &entry.author == author && !entry.skipped)
        {
            #[allow(clippy::cast_possible_truncation)]
            db::build::mark_skipped(pool, session_id, index as u32, true).await?;
        }
    }

    Ok(())
}

/// Builds the scene and renders it, off the async runtime's worker threads.
async fn render_model(
    history: &History,
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
) -> Option<Vec<u8>> {
    let model = history.materialize();
    if model.is_empty() {
        return None;
    }

    let mode = if library.is_available() {
        SceneMode::Meshes
    } else {
        SceneMode::Boxes
    };
    let (triangles, report) = scene::build_scene(&model, library, mode).await;
    if report.is_approximate() {
        tracing::debug!("build: {} placement(s) drawn as boxes", report.boxed);
    }

    let options = RenderOptions {
        width: cfg.render_width,
        height: cfg.render_height,
        supersample: cfg.supersample,
        ..RenderOptions::default()
    };

    // Rendering is CPU-bound; keep it off the gateway's threads and cap how
    // many can run at once.
    let _permit = render_slots().acquire().await.ok()?;
    let triangles: Vec<SceneTriangle> = triangles;
    tokio::task::spawn_blocking(move || crate::render::render_png(&triangles, &options))
        .await
        .ok()?
        .ok()
}

async fn post_render(
    ctx: &Context,
    channel_id: ChannelId,
    history: &History,
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
    session_id: &str,
    locale: &str,
) {
    let _ = channel_id.broadcast_typing(&ctx.http).await;
    let Some(png) = render_model(history, library, cfg).await else {
        return;
    };

    let model = history.materialize();
    let message = CreateMessage::new()
        .content(build_cmd::render_caption(
            &model,
            history.revision(),
            locale,
        ))
        .add_file(CreateAttachment::bytes(png, "model.png"))
        .components(vec![build_cmd::build_session_buttons(
            session_id,
            locale,
            cfg.enable_io_export,
        )]);

    if let Err(e) = channel_id.send_message(&ctx.http, message).await {
        tracing::error!("build: failed to post render: {e}");
    }
}

async fn post_export(
    ctx: &Context,
    channel_id: ChannelId,
    history: &History,
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
    locale: &str,
) {
    let model = history.materialize();
    if model.is_empty() {
        let _ = channel_id
            .say(
                &ctx.http,
                rust_i18n::t!("command.build.export_empty", locale = locale).to_string(),
            )
            .await;
        return;
    }

    let thumbnail = render_model(history, library, cfg)
        .await
        .unwrap_or_default();
    let bytes = super::export::to_io(&model, &thumbnail);
    let name = format!("{}.io", super::export::file_stem(&model.name));

    let message = CreateMessage::new()
        .content(
            rust_i18n::t!(
                "command.build.export_ready",
                locale = locale,
                name = &model.name
            )
            .to_string(),
        )
        .add_file(CreateAttachment::bytes(bytes, name));

    if let Err(e) = channel_id.send_message(&ctx.http, message).await {
        tracing::error!("build: failed to post export: {e}");
    }
}

/// Offers the `/part` search results for a part the language could not
/// resolve.
async fn offer_part_choices(
    ctx: &Context,
    config: &Config,
    channel_id: ChannelId,
    pending: &dispatch::PendingLookup,
    locale: &str,
) -> anyhow::Result<()> {
    let http = crate::http::HttpClient::new();
    let response = crate::commands::part::part_interaction(
        ctx,
        &http,
        &pending.query,
        locale,
        &["bricklink".to_string(), "rebrickable".to_string()],
        config.resource_limits.max_http_body_bytes,
    )
    .await?;

    if let crate::commands::part::PartInteractionResponse::SearchResults(options) = response {
        let menu = serenity::all::CreateSelectMenu::new(
            build_cmd::SELECT_PART,
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
    let locale = app_ctx
        .config
        .locale_for(guild_id)
        .unwrap_or("en-US")
        .to_string();

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
    let library = library_for(ctx, &app_ctx.config).await;

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

/// The shared LDraw library, so its mesh cache is reused across sessions.
async fn library_for(ctx: &Context, config: &Config) -> Arc<LdrawLibrary> {
    if let Some(library) = ctx.data.read().await.get::<crate::LdrawData>() {
        return library.clone();
    }
    // Should not happen once main.rs registers it, but a fresh instance is
    // still correct — only colder.
    Arc::new(LdrawLibrary::new(
        &config.ldraw.data_dir,
        crate::ldraw::resolve::ResolveLimits {
            max_files: config.ldraw.max_files,
            ..crate::ldraw::resolve::ResolveLimits::default()
        },
    ))
}

async fn respond_ephemeral(
    ctx: &Context,
    command: &CommandInteraction,
    content: &str,
) -> anyhow::Result<()> {
    command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

/// How long to wait before a render, given the configured debounce.
#[must_use]
pub const fn debounce_delay(cfg: &BuildCommandConfig) -> Duration {
    Duration::from_millis(cfg.render_debounce_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_waits_until_the_debounce_window_has_passed() {
        // Four people typing at once should produce one picture, not four.
        assert!(!should_render_now(0, 1200));
        assert!(!should_render_now(1199, 1200));
        assert!(should_render_now(1200, 1200));
        assert!(should_render_now(5000, 1200));
    }

    #[test]
    fn a_zero_debounce_always_renders() {
        assert!(should_render_now(0, 0));
    }

    #[test]
    fn the_debounce_delay_comes_from_config() {
        let cfg = BuildCommandConfig {
            render_debounce_ms: 900,
            ..BuildCommandConfig::default()
        };
        assert_eq!(debounce_delay(&cfg), Duration::from_millis(900));
    }

    #[test]
    fn there_are_a_bounded_number_of_render_slots() {
        // The cap is what keeps a busy guild from starving the gateway.
        assert!(render_slots().available_permits() <= 2);
    }
}
