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

mod buttons;
mod drafts;
mod proposals;
mod render;
mod thread;

use buttons::offer_part_choices;
pub use buttons::{handle_build_button, handle_part_choice};
use drafts::{
    DraftContext, Respondable, StoredDraft, create_draft, finish_draft, open_draft, save_draft,
    show_preview,
};
pub use drafts::{
    handle_draft_button, handle_draw_button, handle_draw_modal, handle_insert,
    handle_legend_command,
};
use proposals::{ProposalContext, offer_draft_positions, offer_proposals, place_draft};
pub use proposals::{handle_proposal_button, handle_propose_location_modal};
use render::{cancel_pending_render, post_export, post_render, render_model, schedule_render};
pub use thread::{handle_build_command, handle_thread_message};

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
            .map(|pending| (pending.sentence.as_str(), pending.kind)),
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
mod tests;
