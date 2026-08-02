//! The Discord side of a build session.
//!
//! The only module in [`super`] that touches serenity or sqlx. It is
//! deliberately thin: every decision has already been made by
//! [`super::dispatch`] or [`super::report`], which is where the tests are.
//! What is left here is thread creation, persistence, and the render loop.

#![allow(clippy::doc_markdown, clippy::too_many_lines)]

use super::debug_log::{BuildDebugLog, Classification, MessageContext};
use super::dispatch::{self, Outcome};
use super::draft;
use super::ops::History;
use super::scene::{self, SceneMode};
use super::units::MaxModelSize;
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

/// What to do about a render request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPlan {
    /// Draw immediately.
    Now,
    /// Draw once, after this long, replacing any render already waiting.
    After(Duration),
}

/// Works out when the picture for a command should go out.
///
/// Leading edge plus trailing edge: a lone command draws at once, so the
/// thread feels responsive, while a burst adds exactly one more picture when
/// the window closes rather than one per message. Separated from the Discord
/// call so the pacing itself can be tested without a clock or a gateway.
#[must_use]
pub fn plan_render(since_last: Duration, debounce: Duration) -> RenderPlan {
    #[allow(clippy::cast_possible_truncation)]
    let elapsed_ms = since_last.as_millis().min(u128::from(u64::MAX)) as u64;
    #[allow(clippy::cast_possible_truncation)]
    let debounce_ms = debounce.as_millis().min(u128::from(u64::MAX)) as u64;
    if should_render_now(elapsed_ms, debounce_ms) {
        RenderPlan::Now
    } else {
        RenderPlan::After(debounce.saturating_sub(since_last))
    }
}

/// Whether a message could possibly belong to a build thread.
///
/// This runs for every message in every channel of every guild, so it must be
/// nearly free. A channel-kind check off the in-memory cache rejects
/// essentially all traffic before any database work.
///
/// Returns `None` when the cache has never seen the channel, which callers must
/// treat as "don't know" rather than "no": a restart leaves the cache cold, and
/// answering `false` there silently drops every command in a live build thread
/// until it warms up.
fn cached_is_thread(ctx: &Context, channel_id: ChannelId) -> Option<bool> {
    ctx.cache.channel(channel_id).map(|channel| {
        matches!(
            channel.kind,
            ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
        )
    })
}

/// Whether a channel is definitely not a thread, and so cannot hold a session.
///
/// The cheap negative half of [`cached_is_thread`]: only says yes when the
/// cache actually knows.
fn is_definitely_not_a_thread(ctx: &Context, channel_id: ChannelId) -> bool {
    cached_is_thread(ctx, channel_id) == Some(false)
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
async fn log_thread_message(
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

/// Per-session render pacing: when the last picture went out, and the task
/// waiting to send the next one.
struct RenderState {
    last_render: std::time::Instant,
    pending: Option<tokio::task::JoinHandle<()>>,
}

static RENDER_STATE: std::sync::OnceLock<
    moka::future::Cache<String, Arc<tokio::sync::Mutex<RenderState>>>,
> = std::sync::OnceLock::new();

async fn render_state(session_id: &str) -> Arc<tokio::sync::Mutex<RenderState>> {
    RENDER_STATE
        .get_or_init(|| {
            moka::future::Cache::builder()
                .max_capacity(1_000)
                .time_to_idle(Duration::from_hours(1))
                .build()
        })
        .get_with(session_id.to_string(), async {
            Arc::new(tokio::sync::Mutex::new(RenderState {
                // Long enough ago that the first command in a session renders
                // straight away rather than waiting out a window.
                last_render: std::time::Instant::now()
                    .checked_sub(Duration::from_hours(1))
                    .unwrap_or_else(std::time::Instant::now),
                pending: None,
            }))
        })
        .await
}

/// Drops any render that has not gone out yet.
async fn cancel_pending_render(session_id: &str) {
    let state = render_state(session_id).await;
    let mut state = state.lock().await;
    if let Some(handle) = state.pending.take() {
        handle.abort();
    }
}

/// Renders now, or arranges for one render to cover this command and whatever
/// else arrives in the next `render_debounce_ms`.
///
/// Leading edge plus trailing edge: a lone command draws immediately, so the
/// thread feels responsive, and a burst adds exactly one more picture at the
/// end of the window rather than one per message. The trailing render reloads
/// the history when it fires, so it shows everything that landed meanwhile.
async fn schedule_render(
    ctx: &Context,
    pool: &sqlx::SqlitePool,
    session: &db::build::BuildSession,
    cfg: &BuildCommandConfig,
    locale: &str,
    channel_id: ChannelId,
) {
    let state = render_state(&session.id).await;
    let mut state = state.lock().await;

    if let Some(handle) = state.pending.take() {
        // A later command supersedes the render already waiting.
        handle.abort();
    }

    let wait = match plan_render(state.last_render.elapsed(), debounce_delay(cfg)) {
        RenderPlan::Now => {
            state.last_render = std::time::Instant::now();
            None
        }
        RenderPlan::After(wait) => Some(wait),
    };

    let Some(wait) = wait else {
        drop(state);
        let history = match db::build::load_history(pool, &session.id, &session.model_name).await {
            Ok(history) => history,
            Err(e) => {
                tracing::error!("build: could not load history to render: {e}");
                return;
            }
        };
        let library = library_for(ctx).await;
        post_render(
            ctx,
            channel_id,
            &history,
            &library,
            cfg,
            &session.id,
            locale,
        )
        .await;
        return;
    };

    let ctx = ctx.clone();
    let pool = pool.clone();
    let session = session.clone();
    let cfg = cfg.clone();
    let locale = locale.to_string();
    let state_handle = Arc::clone(&render_state(&session.id).await);

    state.pending = Some(tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        let Ok(history) = db::build::load_history(&pool, &session.id, &session.model_name).await
        else {
            return;
        };
        let library = library_for(&ctx).await;
        post_render(
            &ctx,
            channel_id,
            &history,
            &library,
            &cfg,
            &session.id,
            &locale,
        )
        .await;
        let mut state = state_handle.lock().await;
        state.last_render = std::time::Instant::now();
        state.pending = None;
    }));
}

/// Writes an outcome's operations and cursor movement to the database.
///
/// One transaction, so a crash cannot leave the log truncated but not
/// rewritten. The revision the operations are written at comes from
/// [`Outcome::ops_start`] rather than the loaded cursor: `Undo. Add a brick.`
/// rewinds before it appends, and deriving the index here would put the new
/// operation back on top of the one the undo just freed.
async fn persist(
    pool: &sqlx::SqlitePool,
    session_id: &str,
    history: &History,
    outcome: &Outcome,
    author: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    if let Some(start) = outcome.ops_start {
        #[allow(clippy::cast_possible_truncation)]
        let start = start as u32;
        // A new command after an undo discards the redo tail, exactly as any
        // editor does.
        db::build::truncate_from(&mut *tx, session_id, start).await?;
        for (offset, op) in outcome.ops.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let n = start + offset as u32;
            db::build::append_revision(&mut *tx, session_id, n, author, op).await?;
        }
    }

    if let Some(cursor) = outcome.cursor {
        db::build::set_cursor(&mut *tx, session_id, cursor).await?;
    }

    if let Some(author) = &outcome.skip_last_by {
        if let Some(index) = history.ops[..history.cursor]
            .iter()
            .rposition(|entry| &entry.author == author && !entry.skipped)
        {
            #[allow(clippy::cast_possible_truncation)]
            db::build::mark_skipped(&mut *tx, session_id, index as u32, true).await?;
        }
    }

    // Either park the sentence that is waiting on a part choice, or clear a
    // stale one: any message that got this far without pausing has moved the
    // model on, so replaying an older sentence would no longer mean what its
    // author meant.
    db::build::set_pending_sentence(
        &mut *tx,
        session_id,
        outcome
            .pending_lookup
            .as_ref()
            .map(|pending| pending.sentence.as_str()),
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Builds the directory of people who have edited this model.
///
/// Only the distinct authors already in the log are looked up — a handful per
/// session, from the member cache — because those are the only people
/// `Undo alice's last edit.` can meaningfully name. The message's own author is
/// included so the very first edit of a session is nameable too.
fn builders_of(
    ctx: &Context,
    msg: &serenity::all::Message,
    history: &History,
) -> dispatch::Builders {
    let mut ids: Vec<String> = history
        .ops
        .iter()
        .map(|entry| entry.author.clone())
        .collect();
    ids.push(msg.author.id.get().to_string());
    ids.sort_unstable();
    ids.dedup();

    let mut entries = Vec::with_capacity(ids.len());
    for id in ids {
        let Ok(user_id) = id.parse::<u64>() else {
            continue;
        };
        let user_id = serenity::all::UserId::new(user_id);
        // A guild nickname is what people actually call each other, so prefer
        // it; fall back to the account name.
        let name = msg
            .guild_id
            .and_then(|guild_id| {
                ctx.cache
                    .member(guild_id, user_id)
                    .map(|member| member.display_name().to_string())
            })
            .or_else(|| ctx.cache.user(user_id).map(|user| user.name.clone()));
        if let Some(name) = name {
            entries.push((name, id.clone()));
        }
        // The id itself always resolves, so a session whose members are not
        // cached still has a usable, if unfriendly, handle.
        entries.push((id.clone(), id));
    }

    // "Undo my last edit." is at least as natural as naming yourself, and the
    // speaker is the one person we always know.
    let me = msg.author.id.get().to_string();
    for pronoun in ["me", "my", "mine"] {
        entries.push(((*pronoun).to_string(), me.clone()));
    }

    dispatch::Builders::new(entries)
}

/// Serialises each session's read-decide-write cycle.
///
/// A build session is collaborative by design, so two people committing in the
/// same instant is the expected case, not a rare one. Without this both would
/// load the same cursor, both discard the redo tail from it, and both write at
/// the same revision index — and the first person's brick would vanish with no
/// error anywhere. The lock is per session, so busy threads do not block each
/// other.
///
/// Process-local, and so only a lock while the bot runs as a single process —
/// which the design assumes. See *The bot runs as a single process* in
/// `docs/ADR-003-LDraw-Rendering-And-Studio-Export.md`.
static SESSION_LOCKS: std::sync::OnceLock<
    moka::future::Cache<String, Arc<tokio::sync::Mutex<()>>>,
> = std::sync::OnceLock::new();

/// A session's effective size cap: its own `size`, if it was opened with one,
/// otherwise the guild's configured default.
fn max_size_for(session: &db::build::BuildSession, cfg: &BuildCommandConfig) -> MaxModelSize {
    session
        .max_size()
        .unwrap_or_else(|| MaxModelSize::cube(cfg.max_model_size))
}

async fn session_lock(session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    SESSION_LOCKS
        .get_or_init(|| {
            moka::future::Cache::builder()
                .max_capacity(1_000)
                .time_to_idle(Duration::from_hours(1))
                .build()
        })
        .get_with(session_id.to_string(), async {
            Arc::new(tokio::sync::Mutex::new(()))
        })
        .await
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

    let http = crate::http::HttpClient::new();
    let part = crate::parts_cache::warm::detail(
        app_ctx.parts_cache.as_deref(),
        &http,
        part_num,
        app_ctx.config.resource_limits.max_http_body_bytes,
    )
    .await?;
    let chosen = super::parts::from_rebrickable(
        &part.part_num,
        &part.name,
        &part.external_ids,
        super::model::PartKind::Brick,
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

/// `workflow_sessions` kind for a set of offered positions.
const PROPOSAL_KIND: &str = "build_proposal";

/// How long an offer stands. Short, because it is answered in a moment and a
/// stale one is refused anyway.
const PROPOSAL_TTL_MINS: u32 = 15;

/// The colour a proposed placement is drawn in, so it stands out from the
/// model it would join.
const PROPOSAL_HIGHLIGHT: crate::ldraw::colors::Rgb = crate::ldraw::colors::Rgb::new(255, 0, 220);

/// One tile of the proposal picture.
const TILE_WIDTH: u32 = 320;
const TILE_HEIGHT: u32 = 240;

/// A set of offered positions, waiting on a choice.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct StoredProposal {
    session_id: String,
    /// The positions on offer, in the order they are numbered.
    options: Vec<Vec<super::ops::Placed>>,
    /// The revision they were worked out against. Anything built since and
    /// they no longer mean what the builder was shown.
    based_on: usize,
}

async fn load_proposal(
    app_ctx: &crate::workflows::AppContext,
    proposal_id: &str,
    user_id: &str,
) -> Option<StoredProposal> {
    let payload = db::get_workflow_session_payload(&app_ctx.db, proposal_id, user_id)
        .await
        .ok()??;
    serde_json::from_value(payload).ok()
}

/// Works out where the named parts could go and asks.
///
/// Posted publicly rather than ephemerally, and not by choice: this is
/// triggered by a chat message, which carries no interaction token, and
/// Discord will not send an ephemeral message without one. The part picker
/// (`offer_part_choices`) posts publicly for exactly the same reason. In a
/// thread where everyone is building together it is no great loss.
struct ProposalContext<'a> {
    pool: &'a sqlx::SqlitePool,
    session: &'a db::build::BuildSession,
    user_id: &'a str,
    guild_id: u64,
    channel_id: ChannelId,
    cfg: &'a BuildCommandConfig,
    locale: &'a str,
}

async fn offer_proposals(
    ctx: &Context,
    who: &ProposalContext<'_>,
    pending: &dispatch::PendingProposal,
) -> anyhow::Result<()> {
    let ProposalContext {
        pool,
        session,
        user_id,
        guild_id,
        channel_id,
        cfg,
        locale,
    } = *who;
    let history = db::build::load_history(pool, &session.id, &session.model_name).await?;
    let model = history.materialize();

    let candidates =
        super::propose::propose_any(&model, &pending.batches, build_cmd::MAX_PROPOSALS);
    if candidates.is_empty() {
        channel_id
            .say(
                &ctx.http,
                rust_i18n::t!("command.build.propose_none", locale = locale).to_string(),
            )
            .await?;
        return Ok(());
    }

    let stored = StoredProposal {
        session_id: session.id.clone(),
        options: candidates.iter().map(|c| c.placements.clone()).collect(),
        based_on: history.revision(),
    };
    let proposal_id = db::create_workflow_session(
        pool,
        PROPOSAL_KIND,
        user_id,
        &guild_id.to_string(),
        &channel_id.to_string(),
        serde_json::to_value(&stored)?,
        PROPOSAL_TTL_MINS,
    )
    .await?;

    let library = library_for(ctx).await;
    let picture = render_proposals(&model, &candidates, &library, cfg).await;

    let mut message = CreateMessage::new()
        .content(build_cmd::proposal_message(
            &pending.label,
            &candidates,
            locale,
        ))
        .components(build_cmd::build_proposal_buttons(
            &proposal_id,
            candidates.len(),
            locale,
        ));
    if let Some(png) = picture {
        message = message.add_file(serenity::all::CreateAttachment::bytes(png, "proposals.png"));
    }
    channel_id.send_message(&ctx.http, message).await?;
    Ok(())
}

/// Draws every offer as one picture, tiled.
///
/// All the tiles share **one** camera, fitted to everything any of them
/// contains. Framing each to its own bounds would rescale the model between
/// tiles, and the eye would read that reframing as the difference — which is
/// exactly wrong when the pictures are alternatives to compare.
async fn render_proposals(
    model: &super::Model,
    candidates: &[super::propose::Candidate],
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
) -> Option<Vec<u8>> {
    use crate::math::Aabb;
    use crate::render::{bounds_of, framebuffer_size, render_to_framebuffer_with};

    let mode = if library.is_available() {
        SceneMode::Meshes
    } else {
        SceneMode::Boxes
    };
    let (base, _) = scene::build_scene(model, library, mode).await;

    let soups: Vec<Vec<SceneTriangle>> = candidates
        .iter()
        .map(|candidate| {
            let mut soup = base.clone();
            for placed in &candidate.placements {
                soup.extend(scene::candidate_triangles(
                    &super::model::Placement {
                        id: super::model::PlacementId::new(0, 0),
                        part: placed.part.clone(),
                        color: placed.color,
                        pos: placed.pos,
                        orientation: placed.orientation,
                        group: None,
                    },
                    PROPOSAL_HIGHLIGHT,
                ));
            }
            soup
        })
        .collect();
    if soups.is_empty() {
        return None;
    }

    let options = RenderOptions {
        width: TILE_WIDTH,
        height: TILE_HEIGHT,
        // The tiles are small already; supersampling them costs four times as
        // much for a picture nobody studies closely.
        supersample: 1,
        ..RenderOptions::default()
    };
    let union = soups
        .iter()
        .map(|soup| bounds_of(soup))
        .reduce(|a: Aabb, b: Aabb| Aabb {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        })?;
    let (frame_w, frame_h) = framebuffer_size(&options);
    let camera = crate::render::Camera::fit_default(union, frame_w, frame_h);
    let background = options.background_top;
    let _ = cfg;

    // One permit for the whole sheet, not one per tile: the semaphore is
    // process-wide with two slots, and four separate acquisitions would starve
    // the live renders every other thread is waiting on.
    let _permit = render_slots().acquire().await.ok()?;
    tokio::task::spawn_blocking(move || {
        let columns = 2u32;
        let rows = u32::try_from(soups.len().div_ceil(2)).unwrap_or(1);
        let mut sheet =
            crate::render::Framebuffer::new(TILE_WIDTH * columns, TILE_HEIGHT * rows, background);
        for (index, soup) in soups.iter().enumerate() {
            let tile = render_to_framebuffer_with(soup, &options, camera);
            let index = u32::try_from(index).unwrap_or(0);
            sheet.blit(
                &tile,
                (index % columns) * TILE_WIDTH,
                (index / columns) * TILE_HEIGHT,
            );
        }
        crate::render::encode_png(&sheet)
    })
    .await
    .ok()?
    .ok()
}

const DRAFT_KIND: &str = "build_draft";

/// How long a draft survives without being touched.
const DRAFT_TTL_MINS: u32 = 60;

/// A draft as it is stored between interactions.
///
/// Kept in `workflow_sessions`, which is **owner-scoped** — the draft belongs
/// to one person, where the model belongs to the thread. That is the whole
/// privacy property, and it comes from the existing table rather than any new
/// schema.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct StoredDraft {
    session_id: String,
    view: String,
    legend: String,
    grid: String,
    location: String,
    /// The revision the last preview was taken against.
    based_on: usize,
    /// Bumped on every preview. A render that finishes after the builder has
    /// moved on carries a stale sequence and drops its edit.
    preview_seq: u64,
}

impl StoredDraft {
    fn fields(&self) -> draft::DraftFields {
        draft::DraftFields {
            view: self.view.clone(),
            legend: self.legend.clone(),
            grid: self.grid.clone(),
            location: self.location.clone(),
        }
    }
}

async fn load_draft(
    app_ctx: &crate::workflows::AppContext,
    draft_id: &str,
    user_id: &str,
) -> Option<StoredDraft> {
    let payload = db::get_workflow_session_payload(&app_ctx.db, draft_id, user_id)
        .await
        .ok()??;
    serde_json::from_value(payload).ok()
}

/// The draft this person already has open in this channel, if any.
///
/// A slash command knows only who ran it and where, never a draft id, so this
/// is how `/build-legend` and a repeated `[Draw…]` find the same drawing.
async fn open_draft(
    app_ctx: &crate::workflows::AppContext,
    user_id: &str,
    channel_id: &str,
) -> Option<(String, StoredDraft)> {
    let (draft_id, payload) =
        db::find_latest_workflow_session(&app_ctx.db, DRAFT_KIND, user_id, channel_id)
            .await
            .ok()??;
    let stored: StoredDraft = serde_json::from_value(payload).ok()?;
    Some((draft_id, stored))
}

/// Opens a new draft, prefilled with the model as it stands.
///
/// Editing what is already built is the easiest way to name parts and colours
/// that certainly exist, so a draft starts from the current drawing rather
/// than a blank page.
async fn create_draft(
    app_ctx: &crate::workflows::AppContext,
    session: &db::build::BuildSession,
    user_id: &str,
    guild_id: u64,
    channel_id: &str,
) -> anyhow::Result<(String, StoredDraft)> {
    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let model = history.materialize();
    let fields = super::textview::render(&model, super::units::GridView::Top, None)
        .map(|view| draft::split_fields(&view))
        .unwrap_or_default();

    let stored = StoredDraft {
        session_id: session.id.clone(),
        view: fields.view,
        legend: fields.legend,
        grid: fields.grid,
        location: fields.location,
        based_on: history.revision(),
        preview_seq: 0,
    };
    let draft_id = db::create_workflow_session(
        &app_ctx.db,
        DRAFT_KIND,
        user_id,
        &guild_id.to_string(),
        channel_id,
        serde_json::to_value(&stored).unwrap_or_default(),
        DRAFT_TTL_MINS,
    )
    .await?;
    Ok((draft_id, stored))
}

/// Closes a draft for good, so the next lookup does not find it again.
async fn finish_draft(app_ctx: &crate::workflows::AppContext, draft_id: &str, user_id: &str) {
    let _ = db::consume_workflow_session(&app_ctx.db, draft_id, user_id).await;
}

async fn save_draft(
    app_ctx: &crate::workflows::AppContext,
    draft_id: &str,
    user_id: &str,
    stored: &StoredDraft,
) {
    if let Ok(payload) = serde_json::to_value(stored) {
        let _ = db::update_workflow_session_payload(&app_ctx.db, draft_id, user_id, payload).await;
    }
}

/// Handles the `[Draw…]` button: opens the form, prefilled with the model.
///
/// Starting from what is already built is the easiest way to pick parts that
/// certainly exist, so an established session opens showing itself rather than
/// a blank page.
pub async fn handle_draw_button(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    let channel = interaction.channel_id.get().to_string();
    let Some(session) = db::build::get_active_session_by_channel(&app_ctx.db, &channel).await?
    else {
        return Ok(());
    };
    if session.id != session_id {
        return Ok(());
    }

    // Pressing the button again picks up where you left off rather than
    // starting a second draft: two live drafts in one channel would make
    // "your open drawing" ambiguous for `/build-legend`, and would quietly
    // strand whatever was in the first one.
    if let Some((draft_id, stored)) = open_draft(app_ctx, &user_id, &channel).await {
        return interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Modal(build_cmd::build_draw_modal(
                    &draft_id,
                    &stored.fields(),
                    &locale,
                )),
            )
            .await
            .map_err(Into::into);
    }

    let (draft_id, stored) = create_draft(app_ctx, &session, &user_id, guild_id, &channel).await?;

    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Modal(build_cmd::build_draw_modal(
                &draft_id,
                &stored.fields(),
                &locale,
            )),
        )
        .await?;
    Ok(())
}

/// Handles the two-step legend picker: `[Insert…]`, its pages, and its choices.
///
/// A colour on its own is not a legend line — `symbol = colour part` needs
/// both — so this is one flow in two steps rather than two buttons. The colour
/// already chosen rides in the custom id as its numeric LDraw code, which is
/// short, has no spaces, and cannot collide with the `:` separator.
pub async fn handle_insert(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    base_id: &str,
    arg: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    // `arg` carries everything after the first colon, which each step splits
    // into as many fields as it put there.
    let mut parts = arg.split(':');
    let draft_id = parts.next().unwrap_or_default().to_string();

    let Some(mut stored) = load_draft(app_ctx, &draft_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale.as_str()),
        )
        .await;
    };

    match base_id {
        // Step one: open the colour list.
        build_cmd::BUTTON_INSERT => {
            replace_with(
                ctx,
                interaction,
                build_cmd::build_colour_page(&draft_id, 0, &locale),
            )
            .await
        }
        build_cmd::PAGE_INSERT_COLOUR => {
            let page = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            replace_with(
                ctx,
                interaction,
                build_cmd::build_colour_page(&draft_id, page, &locale),
            )
            .await
        }
        // Step two: a colour is chosen, so show the parts.
        build_cmd::PICK_INSERT_COLOUR => {
            let Some(colour) = selected_value(interaction) else {
                return Ok(());
            };
            replace_with(
                ctx,
                interaction,
                build_cmd::build_part_page(&draft_id, &colour, 0, &locale),
            )
            .await
        }
        build_cmd::PAGE_INSERT_PART => {
            let colour = parts.next().unwrap_or(build_cmd::NO_COLOUR).to_string();
            let page = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            replace_with(
                ctx,
                interaction,
                build_cmd::build_part_page(&draft_id, &colour, page, &locale),
            )
            .await
        }
        // Both chosen: write the line and show the drawing again.
        build_cmd::PICK_INSERT_PART => {
            let colour = parts.next().unwrap_or(build_cmd::NO_COLOUR).to_string();
            let Some(part) = selected_value(interaction) else {
                return Ok(());
            };
            let colour_name = colour_name_of(&colour);

            let mut fields = stored.fields();
            let symbol = draft::next_symbol(&fields.legend, colour_name);
            draft::append_legend_line(&mut fields, symbol, colour_name, &part);
            stored.legend = fields.legend;

            show_preview(
                ctx,
                app_ctx,
                interaction,
                &mut stored,
                &DraftContext {
                    draft_id: &draft_id,
                    user_id: &user_id,
                    locale: &locale,
                    guild_id,
                    channel_id: interaction.channel_id,
                },
            )
            .await
        }
        _ => Ok(()),
    }
}

/// The colour a picker step is carrying, or `None` for "no colour".
fn colour_name_of(code: &str) -> Option<&'static str> {
    if code == build_cmd::NO_COLOUR {
        return None;
    }
    code.parse::<u16>()
        .ok()
        .and_then(crate::ldraw::colors::name_of)
}

/// The single value chosen from a select menu.
fn selected_value(interaction: &ComponentInteraction) -> Option<String> {
    match &interaction.data.kind {
        serenity::all::ComponentInteractionDataKind::StringSelect { values } => {
            values.first().cloned()
        }
        _ => None,
    }
}

/// Swaps the picker's own message for its next step, rather than posting a new
/// one per page.
async fn replace_with(
    ctx: &Context,
    interaction: &ComponentInteraction,
    components: Vec<serenity::all::CreateActionRow>,
) -> anyhow::Result<()> {
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new().components(components),
            ),
        )
        .await?;
    Ok(())
}

/// Handles `/build-legend`: adds a line to the open drawing.
///
/// Opens a draft when there is none, prefilled from the current model — one
/// command should get you from nothing to a drawing with the part you picked
/// already in it. Only the absence of a `/build` session is a refusal, because
/// then there is nothing to draft against.
pub async fn handle_legend_command(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    command: &CommandInteraction,
) -> anyhow::Result<()> {
    let guild_id = command.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = command.user.id.get().to_string();
    let channel = command.channel_id.get().to_string();

    let (mut part, mut colour, mut symbol) = (None, None, None);
    for option in &command.data.options {
        let Some(which) = build_cmd::legend_option(&option.name) else {
            continue;
        };
        let Some(value) = option.value.as_str() else {
            continue;
        };
        match which {
            build_cmd::LegendOption::Part => part = Some(value.to_string()),
            build_cmd::LegendOption::Colour => colour = Some(value.to_string()),
            build_cmd::LegendOption::Symbol => symbol = value.chars().next(),
        }
    }

    let Some(part) = part else {
        return Ok(());
    };

    let Some(session) = db::build::get_active_session_by_channel(&app_ctx.db, &channel).await?
    else {
        return respond_ephemeral_command(
            ctx,
            command,
            &rust_i18n::t!("command.build.legend_no_session", locale = locale.as_str()),
        )
        .await;
    };

    let (draft_id, mut stored) = match open_draft(app_ctx, &user_id, &channel).await {
        Some(found) => found,
        None => create_draft(app_ctx, &session, &user_id, guild_id, &channel).await?,
    };

    let mut fields = stored.fields();
    let symbol = symbol.unwrap_or_else(|| draft::next_symbol(&fields.legend, colour.as_deref()));
    draft::append_legend_line(&mut fields, symbol, colour.as_deref(), &part);
    stored.legend = fields.legend;

    show_preview(
        ctx,
        app_ctx,
        command,
        &mut stored,
        &DraftContext {
            draft_id: &draft_id,
            user_id: &user_id,
            locale: &locale,
            guild_id,
            channel_id: command.channel_id,
        },
    )
    .await
}

/// Handles the drawing form's submission.
///
/// Interprets against a *clone* of the history and never persists, so the
/// thread sees nothing. Answers with the text preview straight away — well
/// inside Discord's three seconds — then edits the picture in when it is ready.
pub async fn handle_draw_modal(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &serenity::all::ModalInteraction,
    draft_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    let Some(mut stored) = load_draft(app_ctx, draft_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale.as_str()),
        )
        .await;
    };

    let fields = read_draw_modal(interaction);
    stored.view = fields.view.clone();
    stored.legend = fields.legend.clone();
    stored.grid = fields.grid.clone();
    stored.location = fields.location.clone();

    show_preview(
        ctx,
        app_ctx,
        interaction,
        &mut stored,
        &DraftContext {
            draft_id,
            user_id: &user_id,
            locale: &locale,
            guild_id,
            channel_id: interaction.channel_id,
        },
    )
    .await
}

/// Reads the four inputs back out of a submitted drawing form.
fn read_draw_modal(interaction: &serenity::all::ModalInteraction) -> draft::DraftFields {
    let mut fields = draft::DraftFields::default();
    for row in &interaction.data.components {
        if let Some(serenity::all::ActionRowComponent::InputText(input)) = row.components.first() {
            let value = input.value.clone().unwrap_or_default();
            match input.custom_id.as_str() {
                "view" => fields.view = value,
                "legend" => fields.legend = value,
                "grid" => fields.grid = value,
                "location" => fields.location = value,
                _ => {}
            }
        }
    }
    fields
}

/// Who is drafting, and where — gathered so [`show_preview`] stays under
/// clippy's argument-count lint, the same way [`MessageContext`] does for the
/// debug log.
struct DraftContext<'a> {
    draft_id: &'a str,
    user_id: &'a str,
    locale: &'a str,
    guild_id: u64,
    channel_id: ChannelId,
}

/// Anything that can answer an interaction with an ephemeral preview.
///
/// The form submit and the `[Re-preview]` button do exactly the same thing
/// from here on, and they arrive as different interaction types.
trait Respondable {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()>;
    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>);
}

impl Respondable for serenity::all::ModalInteraction {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()> {
        self.create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;
        Ok(())
    }

    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>) {
        let _ = self
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new()
                    .content(content)
                    .new_attachment(CreateAttachment::bytes(png, "preview.png")),
            )
            .await;
    }
}

impl Respondable for CommandInteraction {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()> {
        self.create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;
        Ok(())
    }

    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>) {
        let _ = self
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new()
                    .content(content)
                    .new_attachment(CreateAttachment::bytes(png, "preview.png")),
            )
            .await;
    }
}

impl Respondable for ComponentInteraction {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()> {
        self.create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;
        Ok(())
    }

    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>) {
        let _ = self
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new()
                    .content(content)
                    .new_attachment(CreateAttachment::bytes(png, "preview.png")),
            )
            .await;
    }
}

/// Previews a draft and answers with it, then edits the picture in.
async fn show_preview<R: Respondable>(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &R,
    stored: &mut StoredDraft,
    who: &DraftContext<'_>,
) -> anyhow::Result<()> {
    let (draft_id, locale, user_id) = (who.draft_id, who.locale, who.user_id);
    let cfg = app_ctx.config.commands_for(who.guild_id).build;

    let Some(session) =
        db::build::get_active_session_by_channel(&app_ctx.db, &who.channel_id.get().to_string())
            .await?
    else {
        return interaction
            .reply_ephemeral(
                ctx,
                rust_i18n::t!("command.build.draft_expired", locale = locale).to_string(),
                Vec::new(),
            )
            .await;
    };

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let builders = dispatch::Builders::new([(user_id.to_string(), user_id.to_string())]);
    let preview = draft::preview(
        &history,
        &stored.fields(),
        locale,
        &builders,
        user_id,
        max_size_for(&session, &cfg),
    );

    stored.based_on = preview.based_on;
    stored.preview_seq = stored.preview_seq.saturating_add(1);
    save_draft(app_ctx, draft_id, user_id, stored).await;

    let text = build_cmd::draft_preview_message(&preview, locale);
    let buttons = build_cmd::build_draft_buttons(draft_id, locale, false);
    interaction
        .reply_ephemeral(ctx, text.clone(), buttons)
        .await?;

    // A failed drawing has no picture worth drawing.
    if preview.failed() {
        return Ok(());
    }

    // Then the render, which is slow enough that it cannot share the first
    // response. If the builder has moved on by the time it lands, the stored
    // sequence has advanced and the edit is dropped.
    let seq = stored.preview_seq;
    let library = library_for(ctx).await;
    let mut projected = history;
    for op in &preview.outcome.ops {
        projected.push(op.clone(), user_id);
    }
    if let Some(png) = render_model(&projected, &library, &cfg).await {
        if load_draft(app_ctx, draft_id, user_id)
            .await
            .is_some_and(|current| current.preview_seq == seq)
        {
            interaction.attach_render(ctx, text, png).await;
        }
    }
    Ok(())
}

/// Handles the `[Edit]`, `[Apply]`, `[Discard]` and `[Preview again]` buttons.
pub async fn handle_draft_button(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    base_id: &str,
    draft_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    let Some(mut stored) = load_draft(app_ctx, draft_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale.as_str()),
        )
        .await;
    };

    match base_id {
        build_cmd::BUTTON_DRAFT_EDIT => {
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Modal(build_cmd::build_draw_modal(
                        draft_id,
                        &stored.fields(),
                        &locale,
                    )),
                )
                .await?;
            Ok(())
        }
        build_cmd::BUTTON_DRAFT_REPREVIEW => {
            show_preview(
                ctx,
                app_ctx,
                interaction,
                &mut stored,
                &DraftContext {
                    draft_id,
                    user_id: &user_id,
                    locale: &locale,
                    guild_id,
                    channel_id: interaction.channel_id,
                },
            )
            .await
        }
        build_cmd::BUTTON_DRAFT_PLACE => {
            // The drafted block, offered positions instead of taking the one
            // written in the form. Same engine, same picture, bigger batch.
            place_draft(
                ctx,
                app_ctx,
                interaction,
                draft_id,
                &stored,
                &locale,
                &user_id,
            )
            .await
        }
        build_cmd::BUTTON_DRAFT_DISCARD => {
            // Actually close it, or the next `[Draw…]` or `/build-legend`
            // would pick the discarded drawing straight back up.
            finish_draft(app_ctx, draft_id, &user_id).await;
            respond_ephemeral_component(
                ctx,
                interaction,
                &rust_i18n::t!("command.build.draft_discarded", locale = locale.as_str()),
            )
            .await
        }
        build_cmd::BUTTON_DRAFT_APPLY => {
            apply_draft(
                ctx,
                app_ctx,
                interaction,
                draft_id,
                &stored,
                &locale,
                &user_id,
            )
            .await
        }
        _ => Ok(()),
    }
}

/// Commits a draft to the shared model and posts it publicly.
///
/// Refuses when the session has moved since the preview: what the builder saw
/// must be exactly what lands, and a placement id from a stale preview may no
/// longer resolve. The draft is kept so `[Preview again]` can pick it up.
async fn apply_draft(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    draft_id: &str,
    stored: &StoredDraft,
    locale: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale),
        )
        .await;
    };

    let cfg = app_ctx
        .config
        .commands_for(interaction.guild_id.unwrap_or_default().get())
        .build;

    let lock = session_lock(&session.id).await;
    let guard = lock.lock().await;

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let builders = dispatch::Builders::new([(user_id.to_string(), user_id.to_string())]);
    let preview = draft::preview(
        &history,
        &stored.fields(),
        locale,
        &builders,
        user_id,
        max_size_for(&session, &cfg),
    );

    if draft::is_stale(&preview, &history) || stored.based_on != history.revision() {
        drop(guard);
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_stale", locale = locale),
        )
        .await;
    }
    if preview.failed() {
        drop(guard);
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_nothing_to_apply", locale = locale),
        )
        .await;
    }

    persist(
        &app_ctx.db,
        &session.id,
        &history,
        &preview.outcome,
        user_id,
    )
    .await?;
    let _ = db::build::touch_session(&app_ctx.db, &session.id, cfg.session_ttl_hours).await;
    drop(guard);

    // The drawing is in the model now, so the draft is over.
    finish_draft(app_ctx, draft_id, user_id).await;

    respond_ephemeral_component(
        ctx,
        interaction,
        &rust_i18n::t!("command.build.draft_applied", locale = locale),
    )
    .await?;

    // Now it is public, like anything else the thread builds.
    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let library = library_for(ctx).await;
    post_render(
        ctx,
        interaction.channel_id,
        &history,
        &library,
        &cfg,
        &session.id,
        locale,
    )
    .await;
    Ok(())
}

/// Handles the numbered buttons, the cancel, and the "draw this instead" menu
/// under a set of placement proposals.
pub async fn handle_proposal_button(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    base_id: &str,
    arg: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    // `build_place` carries the chosen index after the proposal id.
    let (proposal_id, index_arg) = arg
        .split_once(':')
        .map_or((arg, None), |(id, rest)| (id, Some(rest.to_string())));

    let Some(stored) = load_proposal(app_ctx, proposal_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };

    if base_id == build_cmd::BUTTON_PROPOSE_CANCEL {
        let _ = db::consume_workflow_session(&app_ctx.db, proposal_id, &user_id).await;
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_cancelled", locale = locale.as_str()),
        )
        .await;
    }

    // Which offer: from the button's own id, or from the menu's value.
    let chosen: usize = match base_id {
        build_cmd::SELECT_PROPOSE_DRAW => selected_value(interaction)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        _ => index_arg.and_then(|v| v.parse().ok()).unwrap_or(0),
    };
    let Some(placements) = stored.options.get(chosen).cloned() else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };

    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };
    let cfg = app_ctx.config.commands_for(guild_id).build;

    // "Draw this instead" hands the position to the drawing form rather than
    // building anything, so a proposed brick can grow into a whole block.
    if base_id == build_cmd::SELECT_PROPOSE_DRAW {
        let _ = db::consume_workflow_session(&app_ctx.db, proposal_id, &user_id).await;
        return open_draft_at(
            ctx,
            app_ctx,
            interaction,
            &session,
            &placements,
            (guild_id, &locale, &user_id),
        )
        .await;
    }

    let lock = session_lock(&session.id).await;
    let guard = lock.lock().await;
    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;

    // Someone else building in between moves everything the offer was measured
    // against, so what was shown is no longer what would land.
    if stored.based_on != history.revision() {
        drop(guard);
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    }

    // The placements are already legal — `propose` settled them through
    // `legal` — so they go in as one ordinary operation.
    let outcome = Outcome {
        ops: vec![super::ops::Op::AddMany {
            placements,
            group: None,
        }],
        ops_start: Some(history.revision()),
        cursor: Some(history.revision() + 1),
        ..Outcome::default()
    };
    persist(&app_ctx.db, &session.id, &history, &outcome, &user_id).await?;
    let _ = db::build::touch_session(&app_ctx.db, &session.id, cfg.session_ttl_hours).await;
    drop(guard);

    let _ = db::consume_workflow_session(&app_ctx.db, proposal_id, &user_id).await;
    respond_ephemeral_component(
        ctx,
        interaction,
        &rust_i18n::t!("command.build.propose_placed", locale = locale.as_str()),
    )
    .await?;

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
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
    Ok(())
}

/// Offers positions for the block currently being drafted.
///
/// The drawing is previewed with its own location stripped out, so what comes
/// back is the block in local coordinates — a batch like any other.
async fn place_draft(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    draft_id: &str,
    stored: &StoredDraft,
    locale: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale),
        )
        .await;
    };
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let cfg = app_ctx.config.commands_for(guild_id).build;

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let builders = dispatch::Builders::new([(user_id.to_string(), user_id.to_string())]);
    let mut fields = stored.fields();
    fields.location = String::new();
    let preview = draft::preview(
        &history,
        &fields,
        locale,
        &builders,
        user_id,
        max_size_for(&session, &cfg),
    );

    let Some(pending) = preview.outcome.pending_proposal.clone() else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_nothing_to_apply", locale = locale),
        )
        .await;
    };

    // The draft is done with; what happens next is an ordinary proposal.
    finish_draft(app_ctx, draft_id, user_id).await;
    respond_ephemeral_component(
        ctx,
        interaction,
        &rust_i18n::t!("command.build.propose_pick", locale = locale),
    )
    .await?;
    offer_proposals(
        ctx,
        &ProposalContext {
            pool: &app_ctx.db,
            session: &session,
            user_id,
            guild_id,
            channel_id: interaction.channel_id,
            cfg: &cfg,
            locale,
        },
        &pending,
    )
    .await
}

/// Opens the drawing form prefilled with a proposed placement.
async fn open_draft_at(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    session: &db::build::BuildSession,
    placements: &[super::ops::Placed],
    who: (u64, &str, &str),
) -> anyhow::Result<()> {
    let (guild_id, locale, user_id) = who;
    let channel = interaction.channel_id.get().to_string();
    let (draft_id, mut stored) = match open_draft(app_ctx, user_id, &channel).await {
        Some(found) => found,
        None => create_draft(app_ctx, session, user_id, guild_id, &channel).await?,
    };

    // Start the drawing from the proposal itself: its parts, at the position
    // that was picked, so nothing has to be typed twice.
    let mut sketch = super::Model::new(&session.model_name);
    for (seq, placed) in placements.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        sketch.placements.push(super::model::Placement {
            id: super::model::PlacementId::new(0, seq as u32),
            part: placed.part.clone(),
            color: placed.color,
            pos: placed.pos,
            orientation: placed.orientation,
            group: None,
        });
    }
    if let Some(view) = super::textview::render(&sketch, super::units::GridView::Top, None) {
        let fields = draft::split_fields(&view);
        stored.view = fields.view;
        stored.legend = fields.legend;
        stored.grid = fields.grid;
    }
    // The drawing lands where the proposal was going to.
    if let Some(first) = placements.first() {
        stored.location = format!("at {} {} {}", first.pos.x, first.pos.y, first.pos.z);
    }
    save_draft(app_ctx, &draft_id, user_id, &stored).await;

    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Modal(build_cmd::build_draw_modal(
                &draft_id,
                &stored.fields(),
                locale,
            )),
        )
        .await?;
    Ok(())
}

async fn respond_ephemeral_command(
    ctx: &Context,
    command: &CommandInteraction,
    content: &str,
) -> anyhow::Result<()> {
    command
        .reply_ephemeral(ctx, content.to_string(), Vec::new())
        .await
}

async fn respond_ephemeral_component<R: Respondable>(
    ctx: &Context,
    interaction: &R,
    content: &str,
) -> anyhow::Result<()> {
    interaction
        .reply_ephemeral(ctx, content.to_string(), Vec::new())
        .await
}

/// The sticky part cache, when one is open.
///
/// Read from the context the same way [`library_for`] reads the LDraw library,
/// because the thread-message path has no `AppContext` to carry it.
async fn parts_cache_for(ctx: &Context) -> Option<Arc<crate::parts_cache::PartCache>> {
    let data = ctx.data.read().await;
    data.get::<crate::PartsCacheData>()
        .cloned()
        .unwrap_or_default()
}

/// The shared LDraw library, so its mesh cache is reused across sessions.
///
/// Reads everything it needs from the context, so a spawned render task does
/// not have to carry a config along with it.
async fn library_for(ctx: &Context) -> Arc<LdrawLibrary> {
    // Take everything wanted from the map, then let the read guard go: what
    // follows can allocate and must not hold a lock while it does.
    let (library, ldraw) = {
        let data = ctx.data.read().await;
        (
            data.get::<crate::LdrawData>().cloned(),
            data.get::<crate::ConfigData>()
                .map(|config| config.ldraw.clone())
                .unwrap_or_default(),
        )
    };
    if let Some(library) = library {
        return library;
    }
    // Should not happen once main.rs registers it, but a fresh instance is
    // still correct — only colder.
    Arc::new(LdrawLibrary::new(
        &ldraw.data_dir,
        crate::ldraw::resolve::ResolveLimits {
            max_files: ldraw.max_files,
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
    fn a_lone_command_draws_straight_away() {
        // Leading edge: the thread must not feel sluggish when one person is
        // building on their own.
        assert_eq!(
            plan_render(Duration::from_secs(30), Duration::from_millis(1200)),
            RenderPlan::Now
        );
    }

    #[test]
    fn a_burst_is_coalesced_into_one_later_render() {
        // Trailing edge: each command inside the window replaces the render
        // waiting to go out, so four people typing at once get one picture.
        assert_eq!(
            plan_render(Duration::from_millis(0), Duration::from_millis(1200)),
            RenderPlan::After(Duration::from_millis(1200))
        );
        assert_eq!(
            plan_render(Duration::from_millis(900), Duration::from_millis(1200)),
            RenderPlan::After(Duration::from_millis(300)),
            "the wait shrinks as the window runs down"
        );
    }

    #[test]
    fn a_zero_debounce_never_waits() {
        assert_eq!(plan_render(Duration::ZERO, Duration::ZERO), RenderPlan::Now);
    }

    #[tokio::test]
    async fn a_fresh_session_renders_immediately() {
        let state = render_state("debounce-fresh").await;
        let since = state.lock().await.last_render.elapsed();
        assert_eq!(
            plan_render(since, Duration::from_millis(1200)),
            RenderPlan::Now,
            "the first command of a session must not wait out a window"
        );
    }

    #[tokio::test]
    async fn cancelling_clears_the_waiting_render() {
        let state = render_state("debounce-cancel").await;
        {
            let mut state = state.lock().await;
            state.pending = Some(tokio::spawn(async {
                tokio::time::sleep(Duration::from_mins(1)).await;
            }));
        }
        cancel_pending_render("debounce-cancel").await;
        assert!(state.lock().await.pending.is_none());
    }

    #[test]
    fn there_are_a_bounded_number_of_render_slots() {
        // The cap is what keeps a busy guild from starving the gateway.
        assert!(render_slots().available_permits() <= 2);
    }

    // --- Persistence -------------------------------------------------------

    async fn setup_db() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    /// Loads, interprets and persists one message, the way
    /// [`handle_thread_message`] does but without a gateway.
    async fn build(pool: &sqlx::SqlitePool, session_id: &str, author: &str, text: &str) {
        let history = db::build::load_history(pool, session_id, "Castle")
            .await
            .unwrap();
        let builders = dispatch::Builders::new([
            ("alice".to_string(), "111".to_string()),
            ("bob".to_string(), "222".to_string()),
        ]);
        let outcome = dispatch::process_message(
            &history,
            text,
            "en-US",
            &builders,
            author,
            MaxModelSize::cube(10),
        );
        persist(pool, session_id, &history, &outcome, author)
            .await
            .unwrap();
    }

    async fn model_of(pool: &sqlx::SqlitePool, session_id: &str) -> crate::build::Model {
        db::build::load_history(pool, session_id, "Castle")
            .await
            .unwrap()
            .materialize()
    }

    #[tokio::test]
    async fn an_undo_then_a_command_keeps_the_new_brick() {
        // Regression: `persist` derived the write position from the loaded
        // cursor, so the new operation landed on top of the one the undo had
        // just freed and the cursor was then set below it — the yellow brick
        // disappeared.
        let pool = setup_db().await;
        let s = db::build::create_build_session(&pool, "g", "t", "alice", "Castle", 24, None)
            .await
            .unwrap();

        build(&pool, &s.id, "alice", "Add a red 2x4 brick at origin.").await;
        build(&pool, &s.id, "bob", "Add a blue 2x4 brick.").await;
        build(&pool, &s.id, "carol", "Undo. Add a yellow 2x4 brick.").await;

        let model = model_of(&pool, &s.id).await;
        assert_eq!(model.len(), 2);
        assert_eq!(model.placements[0].color, 4, "the red brick is untouched");
        assert_eq!(model.placements[1].color, 14, "the yellow brick survived");
    }

    #[tokio::test]
    async fn undo_and_redo_round_trip_through_the_database() {
        let pool = setup_db().await;
        let s = db::build::create_build_session(&pool, "g", "t", "alice", "Castle", 24, None)
            .await
            .unwrap();

        build(&pool, &s.id, "alice", "Add a red 2x4 brick at origin.").await;
        build(&pool, &s.id, "alice", "Add a blue 2x4 brick.").await;
        assert_eq!(model_of(&pool, &s.id).await.len(), 2);

        build(&pool, &s.id, "alice", "Undo.").await;
        assert_eq!(model_of(&pool, &s.id).await.len(), 1);

        build(&pool, &s.id, "alice", "Redo.").await;
        assert_eq!(
            model_of(&pool, &s.id).await.len(),
            2,
            "redo brings the brick back"
        );
    }

    #[tokio::test]
    async fn a_paused_sentence_is_parked_and_later_cleared() {
        let pool = setup_db().await;
        let s = db::build::create_build_session(&pool, "g", "t", "alice", "Castle", 24, None)
            .await
            .unwrap();

        // A part the catalogue cannot answer parks the sentence for the
        // dropdown to pick up.
        build(&pool, &s.id, "alice", "Add a red 3x12 brick at origin.").await;
        let parked = db::build::get_active_session_by_channel(&pool, "t")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parked.pending_sentence.as_deref(),
            Some("Add a red 3x12 brick at origin")
        );
        assert!(
            model_of(&pool, &s.id).await.is_empty(),
            "nothing applied yet"
        );

        // Anything that does go through moves the model on, so the parked
        // sentence is no longer meaningful.
        build(&pool, &s.id, "bob", "Add a blue 2x4 brick at origin.").await;
        assert!(
            db::build::get_active_session_by_channel(&pool, "t")
                .await
                .unwrap()
                .unwrap()
                .pending_sentence
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_session_lock_serialises_writers() {
        // The lock is the only thing standing between two simultaneous
        // builders and a lost brick, so assert it actually excludes.
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let counter = Arc::clone(&counter);
                let peak = Arc::clone(&peak);
                tokio::spawn(async move {
                    let lock = session_lock("session-a").await;
                    let _guard = lock.lock().await;
                    let inside = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    peak.fetch_max(inside, std::sync::atomic::Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(
            peak.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "only one writer at a time"
        );
    }

    #[tokio::test]
    async fn different_sessions_do_not_block_each_other() {
        let a = session_lock("session-b").await;
        let b = session_lock("session-c").await;
        let _held = a.lock().await;
        // Would deadlock if the two sessions shared a lock.
        assert!(b.try_lock().is_ok());
        assert!(Arc::ptr_eq(&a, &session_lock("session-b").await));
    }
}
