#![allow(deprecated, unused_mut)]
rust_i18n::i18n!("locales", fallback = "en-US");

mod ambient;
mod brick;
pub mod build;
mod commands;
pub mod config;
pub mod db;
pub mod events_sync;
pub mod http;
mod i18n;
pub mod interactions;
mod ldraw_sync;
mod links;
mod notifications;
pub mod parts_cache;
mod pocketbase;
pub mod reload;
mod rss;
mod workflows;
mod zulip;

pub use brick_render::{ldraw, math, render};
pub use bricktalk::discord_limits;

use clap::Parser;
use config::Config;
use serenity::all::{
    Client, Command, Context, CreateMessage, EventHandler, GatewayIntents, GuildId, Interaction,
    Message, Ready, async_trait,
};
use serenity::prelude::*;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::env;
use std::str::FromStr;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    #[arg(long, help = "Validate configuration and exit")]
    check_config: bool,

    #[arg(
        long,
        help = "Download the LDraw part library into the configured data directory and exit"
    )]
    fetch_ldraw: bool,

    #[arg(
        long,
        help = "Append a JSONL trace of every /build thread message and how BrickTalk resolved \
                it, for offline grammar tuning. Off by default."
    )]
    build_debug_log: Option<String>,
}

struct Handler {
    config: Config,
    pool: SqlitePool,
    user_cooldowns: moka::future::Cache<String, ()>,
    build_debug_log: Option<std::sync::Arc<crate::build::debug_log::BuildDebugLog>>,
}

pub struct ConfigData;
impl TypeMapKey for ConfigData {
    type Value = std::sync::Arc<Config>;
}

pub struct DbData;
impl TypeMapKey for DbData {
    type Value = SqlitePool;
}

pub struct HttpData;
impl TypeMapKey for HttpData {
    type Value = std::sync::Arc<dyn crate::http::HttpProvider>;
}

/// The sticky part cache, in its own database.
///
/// Absent when it could not be opened or was switched off, in which case
/// autocomplete falls back to the built-in catalogue rather than failing.
pub struct PartsCacheData;
impl TypeMapKey for PartsCacheData {
    type Value = Option<std::sync::Arc<crate::parts_cache::PartCache>>;
}

pub struct SyncMutexData;
impl TypeMapKey for SyncMutexData {
    type Value = std::sync::Arc<tokio::sync::Mutex<()>>;
}

/// The shared `LDraw` part library. Held once so its parsed-file and
/// flattened-mesh caches are reused across every build session rather than
/// rebuilt per render.
pub struct LdrawData;
impl TypeMapKey for LdrawData {
    type Value = std::sync::Arc<crate::ldraw::resolve::LdrawLibrary>;
}

#[must_use]
pub fn is_guild_allowed(config: &crate::config::Config, guild_id: u64) -> bool {
    config.guilds.iter().any(|g| g.guild_id == guild_id)
}

/// The slash commands enabled for `guild_id`, each built in that guild's locale.
fn build_guild_commands(
    config: &crate::config::Config,
    guild_id: u64,
) -> Vec<serenity::all::CreateCommand> {
    let locale = config.locale_or_default(guild_id);
    let cmds = config.commands_for(guild_id);
    let mut commands = Vec::new();

    // Events alone is read globally rather than per guild; guild overrides still gate the
    // interaction handler, so a guild that disables it registers the command but never runs it.
    if config.commands.events.enabled {
        commands.push(crate::commands::events::build_events_command(locale));
    }
    if cmds.locations.enabled {
        commands.push(crate::commands::locations::build_locations_command(locale));
    }
    if cmds.set.enabled {
        commands.push(crate::commands::set::build_set_command(locale));
    }
    if cmds.part.enabled {
        commands.push(crate::commands::part::build_part_command(locale));
    }
    if cmds.diagnostics.enabled {
        commands.push(crate::commands::diagnostics::build_diagnostics_command(
            locale,
        ));
    }
    if cmds.build.enabled {
        commands.push(crate::commands::build::build_build_command(locale));
        // Part of drafting rather than a separable feature, so it rides on the
        // same flag.
        commands.push(crate::commands::build::build_legend_command(locale));
    }

    commands
}
#[allow(clippy::too_many_lines)]
#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("{} is connected!", ready.user.name);

        // Clear global commands to ensure we don't have duplicates
        if let Err(e) = Command::set_global_commands(&ctx.http, vec![]).await {
            tracing::error!("Failed to clear global commands: {}", e);
        }

        for guild in &self.config.guilds {
            let commands = build_guild_commands(&self.config, guild.guild_id);

            match GuildId::new(guild.guild_id)
                .set_commands(&ctx.http, commands)
                .await
            {
                Ok(_) => tracing::info!("Successfully set guild commands for {}", guild.guild_id),
                Err(e) => {
                    tracing::error!("Failed to set guild commands for {}: {}", guild.guild_id, e);
                }
            }
        }
    }

    async fn guild_create(&self, ctx: Context, guild: serenity::all::Guild, _is_new: Option<bool>) {
        let guild_id = guild.id.get();

        if is_guild_allowed(&self.config, guild_id) {
            tracing::info!(
                "Connected to authorized guild: {} ({})",
                guild.name,
                guild_id
            );
        } else {
            tracing::warn!(
                "Joined unauthorized guild {}, leaving immediately.",
                guild_id
            );
            if let Err(e) = ctx.http.leave_guild(guild.id).await {
                tracing::error!("Failed to leave unauthorized guild {}: {}", guild_id, e);
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot || msg.webhook_id.is_some() {
            return;
        }

        // Build sessions live in threads, and a thread has its own channel id,
        // so it is never in `ambient_channel_ids`. This lookup must therefore
        // come *before* that gate below, or every build command would be
        // silently dropped. Returning early also keeps build traffic out of the
        // ambient assistant, whose two-hour per-user cooldown would otherwise
        // throttle a builder to one response every two hours.
        if let Some(guild_id) = msg.guild_id {
            if is_guild_allowed(&self.config, guild_id.get())
                && self.config.commands_for(guild_id.get()).build.enabled
            {
                match crate::build::session::handle_thread_message(
                    &ctx,
                    &self.pool,
                    &self.config,
                    &msg,
                    self.build_debug_log.as_deref(),
                )
                .await
                {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(e) => {
                        tracing::error!("build session error: {e}");
                        return;
                    }
                }
            }
        }

        if let Some(guild_id) = msg.guild_id {
            let guild_id_u64 = guild_id.get();
            if let Some(guild_config) = self
                .config
                .guilds
                .iter()
                .find(|g| g.guild_id == guild_id_u64)
            {
                let channel_id_u64 = msg.channel_id.get();

                // Align ambient assistant with the consent/noise design
                if let Some(ambient_ids) = &guild_config.ambient_channel_ids {
                    if !ambient_ids.contains(&channel_id_u64) {
                        return; // Ignore if not in explicit ambient channels
                    }
                }
            }
        } else {
            // It's a Direct Message
            let zulip_info = if let Some(ref_msg) = &msg.referenced_message {
                crate::db::get_input_topic_by_discord_message_id(
                    &self.pool,
                    &ref_msg.id.get().to_string(),
                )
                .await
                .unwrap_or(None)
            } else {
                None
            };

            let locale = self.config.default_locale().to_string();

            if let Some((topic, stream, payload_str)) = zulip_info {
                if let Some(zulip_cfg) = &self.config.zulip {
                    let user_replied = rust_i18n::t!(
                        "command.zulip.user_replied",
                        locale = locale.as_str(),
                        msg = msg.content
                    );

                    let http_client = std::sync::Arc::new(crate::http::HttpClient::new());
                    let _ = crate::zulip::api::post_topic_to_stream(
                        http_client.as_ref(),
                        zulip_cfg,
                        &stream,
                        &topic,
                        &user_replied,
                        self.config.resource_limits.max_http_body_bytes,
                    )
                    .await;

                    let mut event_title = String::new();
                    if let Ok(payload_json) =
                        serde_json::from_str::<serde_json::Value>(&payload_str)
                    {
                        if let Some(t) = payload_json.get("title").and_then(|v| v.as_str()) {
                            event_title = t.to_string();
                        }
                    }

                    let dm_reply_sent_key =
                        crate::zulip::moderation::ModerationKind::from_topic(&topic)
                            .map_or("command.events.dm_reply_sent", |(kind, _)| {
                                kind.dm_reply_sent_key()
                            });
                    let confirmation = rust_i18n::t!(
                        dm_reply_sent_key,
                        locale = locale.as_str(),
                        title = event_title
                    );
                    let _ = msg.reply(&ctx.http, confirmation).await;
                }
            } else {
                let err_msg =
                    rust_i18n::t!("command.events.dm_reply_no_topic", locale = locale.as_str());
                let _ = msg.reply(&ctx.http, err_msg).await;
            }
            return;
        }

        // Ambient assistant
        let log_ambient = self.config.ambient_debug_logging;
        if let Some(detection) = crate::ambient::detect_topic(&msg.content, log_ambient) {
            let user_id = msg.author.id.get().to_string();
            let is_ignored = crate::db::is_user_ambient_ignored(&self.pool, &user_id)
                .await
                .unwrap_or(false);

            if is_ignored {
                return;
            }

            if self.user_cooldowns.get(&user_id).await.is_some() {
                return;
            }

            if detection.confidence >= crate::ambient::Confidence::Medium {
                let topic_str = format!("{:?}", detection.topic);

                let guild_id_u64 = msg.guild_id.unwrap_or_default().get();
                let log_training_data =
                    self.config
                        .ambient_training_data
                        .as_ref()
                        .map_or(false, |cfg| {
                            if !cfg.enabled {
                                return false;
                            }
                            cfg.allowed_guild_ids
                                .as_ref()
                                .is_none_or(|allowed| allowed.contains(&guild_id_u64))
                        });

                if log_training_data {
                    let is_opt_out = crate::db::is_user_training_opt_out(&self.pool, &user_id)
                        .await
                        .unwrap_or(false);

                    if !is_opt_out {
                        let training_cfg = self.config.ambient_training_data.as_ref().unwrap();
                        let content_to_store = if training_cfg.store_raw_text {
                            Some(msg.content.as_str())
                        } else {
                            None
                        };

                        let guild_pseudo = guild_id_u64.to_string();
                        let channel_pseudo = msg.channel_id.get().to_string();

                        if let Err(e) = crate::db::log_ambient_detection(
                            &self.pool,
                            content_to_store,
                            &topic_str,
                            match detection.confidence {
                                crate::ambient::Confidence::Low => 0.3,
                                crate::ambient::Confidence::Medium => 0.6,
                                crate::ambient::Confidence::High => 0.9,
                            },
                            &guild_pseudo,
                            &channel_pseudo,
                            detection.extracted_id.as_deref(),
                            training_cfg.retention_days,
                        )
                        .await
                        {
                            tracing::error!("Failed to log ambient detection: {}", e);
                        }
                    }
                }

                // Check cooldown
                let item_id_str = detection.extracted_id.clone().unwrap_or_default();

                let mut on_item_cooldown = false;
                if !item_id_str.is_empty() {
                    let channel_id_i64 = i64::try_from(msg.channel_id.get()).unwrap_or_default();
                    let row = crate::db::get_item_cooldown(
                        &self.pool,
                        channel_id_i64,
                        &topic_str,
                        &item_id_str,
                    )
                    .await
                    .unwrap_or(None);
                    if let Some(last_suggested_at) = row {
                        // 12 hours cooldown for the exact same item
                        if chrono::Utc::now().naive_utc() - last_suggested_at
                            < chrono::Duration::hours(12)
                        {
                            on_item_cooldown = true;
                            if log_ambient {
                                tracing::info!(
                                    "Item {:?} is ON COOLDOWN for topic {:?}. Last suggested at: {}",
                                    item_id_str,
                                    detection.topic,
                                    last_suggested_at
                                );
                            }
                        } else if log_ambient {
                            tracing::info!(
                                "Item {:?} cooldown expired. Last suggested at: {}",
                                item_id_str,
                                last_suggested_at
                            );
                        }
                    } else if log_ambient {
                        tracing::info!(
                            "Item {:?} has no previous suggestion history in this channel.",
                            item_id_str
                        );
                    }
                }

                if on_item_cooldown {
                    return;
                }

                let channel_id_i64 = i64::try_from(msg.channel_id.get()).unwrap_or_default();
                let row = crate::db::get_topic_cooldown(&self.pool, channel_id_i64, &topic_str)
                    .await
                    .unwrap_or(None);

                let mut on_cooldown = false;
                if let Some(last_suggested_at) = row {
                    if chrono::Utc::now().naive_utc() - last_suggested_at
                        < chrono::Duration::minutes(30)
                    {
                        on_cooldown = true;
                        if log_ambient {
                            tracing::info!(
                                "Topic {:?} is ON COOLDOWN. Last suggested at: {}",
                                detection.topic,
                                last_suggested_at
                            );
                        }
                    } else if log_ambient {
                        tracing::info!(
                            "Topic {:?} cooldown expired. Last suggested at: {}",
                            detection.topic,
                            last_suggested_at
                        );
                    }
                } else if log_ambient {
                    tracing::info!(
                        "Topic {:?} has no previous suggestion history in this channel.",
                        detection.topic
                    );
                }

                if !on_cooldown {
                    let is_enabled = match detection.topic {
                        crate::ambient::Topic::LegoSet => {
                            self.config.interactions_for(guild_id_u64).set
                        }
                        crate::ambient::Topic::LegoPart => {
                            self.config.interactions_for(guild_id_u64).part
                        }
                    };

                    if !is_enabled {
                        if log_ambient {
                            tracing::info!("Topic {:?} is disabled in config.", detection.topic);
                        }
                        return;
                    }

                    let mut item_name = None;
                    let mut article_count = 0;

                    if let Some(id) = detection.extracted_id.as_deref() {
                        match detection.topic {
                            crate::ambient::Topic::LegoSet => {
                                let http = crate::http::HttpClient::new();
                                if let Ok(set) = crate::brick::fetch_set(
                                    &http,
                                    id,
                                    self.config.resource_limits.max_http_body_bytes,
                                )
                                .await
                                {
                                    item_name = Some(set.name.clone());
                                    if let Ok(articles) = crate::db::search_feed_items(
                                        &self.pool, id, &set.name, &set.theme,
                                    )
                                    .await
                                    {
                                        article_count = articles.len();
                                    }
                                } else {
                                    if log_ambient {
                                        tracing::info!(
                                            "Set {} not found in API, skipping suggestion",
                                            id
                                        );
                                    }
                                    return;
                                }
                            }
                            crate::ambient::Topic::LegoPart => {
                                let http = crate::http::HttpClient::new();
                                // Through the sticky cache, so even a part
                                // merely mentioned in chat is remembered.
                                let cache = {
                                    let data = ctx.data.read().await;
                                    data.get::<PartsCacheData>().cloned().unwrap_or_default()
                                };
                                if let Ok(part) = crate::parts_cache::warm::detail(
                                    cache.as_deref(),
                                    &http,
                                    id,
                                    self.config.resource_limits.max_http_body_bytes,
                                )
                                .await
                                {
                                    item_name = Some(part.name);
                                } else {
                                    if log_ambient {
                                        tracing::info!(
                                            "Part {} not found in API, skipping suggestion",
                                            id
                                        );
                                    }
                                    return;
                                }
                            }
                        }
                    }

                    if log_ambient {
                        tracing::info!("Triggering ambient suggestion for {:?}", detection.topic);
                    }
                    let _ =
                        crate::db::set_topic_cooldown(&self.pool, channel_id_i64, &topic_str).await;

                    if !item_id_str.is_empty() {
                        let _ = crate::db::set_item_cooldown(
                            &self.pool,
                            channel_id_i64,
                            &topic_str,
                            &item_id_str,
                        )
                        .await;
                    }

                    let locale = self
                        .config
                        .locale_or_default(msg.guild_id.unwrap_or_default().get())
                        .to_string();

                    let text = crate::workflows::generate_suggestion_text(
                        detection.topic,
                        &locale,
                        detection.extracted_id.as_deref(),
                        item_name.as_deref(),
                        article_count,
                    );
                    let components = crate::workflows::generate_suggestion_components(
                        detection.topic,
                        &locale,
                        detection.extracted_id.as_deref(),
                        msg.channel_id.get(),
                        msg.id.get(),
                    );

                    let builder = CreateMessage::new()
                        .content(text)
                        .components(components)
                        .reference_message(&msg)
                        .allowed_mentions(serenity::builder::CreateAllowedMentions::new());

                    self.user_cooldowns.insert(user_id, ()).await;

                    if let Err(e) = msg.channel_id.send_message(&ctx.http, builder).await {
                        tracing::error!("Error sending ambient suggestion reply: {}", e);
                    }
                }
            }
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Component(component) => {
                if let Err(e) =
                    crate::workflows::handle_component_interaction(&ctx, &component).await
                {
                    tracing::error!("Error handling component interaction: {}", e);
                }
            }
            Interaction::Modal(modal) => {
                if let Err(e) = crate::workflows::handle_modal_submit(&ctx, &modal).await {
                    tracing::error!("Error handling modal submit: {}", e);
                }
            }
            Interaction::Command(command)
                if (command.data.name.as_str() == "events"
                    || command.data.name.as_str() == "tapahtumat")
                    && self
                        .config
                        .commands_for(command.guild_id.unwrap_or_default().get())
                        .events
                        .enabled =>
            {
                if let Err(e) = crate::workflows::handle_events_wizard_command(&ctx, &command).await
                {
                    tracing::error!("Error handling events wizard command: {}", e);
                }
            }
            Interaction::Command(command)
                if (command.data.name.as_str() == "locations"
                    || command.data.name.as_str() == "paikat")
                    && self
                        .config
                        .commands_for(command.guild_id.unwrap_or_default().get())
                        .locations
                        .enabled =>
            {
                if let Err(e) =
                    crate::workflows::handle_locations_wizard_command(&ctx, &command).await
                {
                    tracing::error!("Error handling locations wizard command: {}", e);
                }
            }
            Interaction::Command(command)
                if (command.data.name.as_str() == "set"
                    || command.data.name.as_str() == "setti")
                    && self
                        .config
                        .commands_for(command.guild_id.unwrap_or_default().get())
                        .set
                        .enabled =>
            {
                let app_ctx = crate::workflows::AppContext::from_serenity_ctx(&ctx).await;
                if let Err(e) =
                    crate::workflows::search::handle_set_command(&ctx, &app_ctx, &command).await
                {
                    tracing::error!("Error handling set command: {}", e);
                }
            }
            Interaction::Command(command)
                if (command.data.name.as_str() == "part"
                    || command.data.name.as_str() == "pala")
                    && self
                        .config
                        .commands_for(command.guild_id.unwrap_or_default().get())
                        .part
                        .enabled =>
            {
                let app_ctx = crate::workflows::AppContext::from_serenity_ctx(&ctx).await;
                if let Err(e) =
                    crate::workflows::search::handle_part_command(&ctx, &app_ctx, &command).await
                {
                    tracing::error!("Error handling part command: {}", e);
                }
            }
            Interaction::Command(command)
                if (command.data.name.as_str() == "diagnostics"
                    || command.data.name.as_str() == "diagnostiikka")
                    && self
                        .config
                        .commands_for(command.guild_id.unwrap_or_default().get())
                        .diagnostics
                        .enabled =>
            {
                let app_ctx = crate::workflows::AppContext::from_serenity_ctx(&ctx).await;
                let locale = self
                    .config
                    .locale_or_default(command.guild_id.unwrap_or_default().get())
                    .to_string();
                if let Err(e) = crate::commands::diagnostics::handle_diagnostics_command(
                    &ctx, &app_ctx, &command, &locale,
                )
                .await
                {
                    tracing::error!("Error handling diagnostics command: {}", e);
                }
            }
            Interaction::Command(command)
                if (command.data.name.as_str() == "build"
                    || command.data.name.as_str() == "rakenna")
                    && self
                        .config
                        .commands_for(command.guild_id.unwrap_or_default().get())
                        .build
                        .enabled =>
            {
                let app_ctx = crate::workflows::AppContext::from_serenity_ctx(&ctx).await;
                if let Err(e) =
                    crate::build::session::handle_build_command(&ctx, &app_ctx, &command).await
                {
                    tracing::error!("Error handling build command: {}", e);
                }
            }
            // Must sit before the autocomplete arm, or it never fires. The
            // name is exact, so it cannot collide with `build` above.
            Interaction::Command(command)
                if (command.data.name.as_str() == "build-legend"
                    || command.data.name.as_str() == "selite")
                    && self
                        .config
                        .commands_for(command.guild_id.unwrap_or_default().get())
                        .build
                        .enabled =>
            {
                let app_ctx = crate::workflows::AppContext::from_serenity_ctx(&ctx).await;
                if let Err(e) =
                    crate::build::session::handle_legend_command(&ctx, &app_ctx, &command).await
                {
                    tracing::error!("Error handling build-legend command: {}", e);
                }
            }
            Interaction::Autocomplete(autocomplete) => {
                if let Err(e) = handle_autocomplete(&ctx, &autocomplete).await {
                    tracing::error!("Error handling autocomplete: {}", e);
                }
            }
            _ => {}
        }
    }
}

/// Answers a typed-ahead suggestion request.
///
/// Discord gives three seconds and no defer, so this **never** waits for the
/// network: colours come from the compiled-in palette, parts from the
/// catalogue plus whatever the sticky cache already knows. A query the cache
/// has not seen starts a background fetch and is answered with what is on
/// hand, so the next keystroke is better than this one.
async fn handle_autocomplete(
    ctx: &Context,
    interaction: &serenity::all::CommandInteraction,
) -> anyhow::Result<()> {
    use crate::commands::autocomplete;

    let Some(focused) = interaction.data.autocomplete() else {
        return Ok(());
    };

    let choices = match focused.name {
        "colour" | "color" | "vari" => autocomplete::colour_choices(focused.value),
        // `/part`'s own argument, under each locale's name for it.
        "part" | "osa" | "query" | "haku" => {
            let (cache, config) = {
                let data = ctx.data.read().await;
                (
                    data.get::<PartsCacheData>().cloned().unwrap_or_default(),
                    data.get::<ConfigData>().cloned(),
                )
            };

            let cached = match &cache {
                Some(cache) => cache.matches(focused.value, 25).await.unwrap_or_default(),
                None => Vec::new(),
            };
            let choices = autocomplete::part_choices(focused.value, &cached);

            // Fill the cache for next time. Returns immediately; the guards
            // inside stop a burst of keystrokes becoming a burst of requests.
            if let (Some(cache), Some(config)) = (cache, config) {
                crate::parts_cache::warm::spawn_warm(
                    cache,
                    std::sync::Arc::new(crate::http::HttpClient::new()),
                    focused.value.to_string(),
                    &config.parts_cache,
                    config.resource_limits.max_http_body_bytes,
                );
            }
            choices
        }
        // An option we do not suggest for still needs an answer, or the
        // client shows an error rather than simply nothing.
        _ => Vec::new(),
    };

    let mut response = serenity::all::CreateAutocompleteResponse::new();
    for choice in choices {
        response = response.add_string_choice(choice.name, choice.value);
    }
    interaction
        .create_response(
            &ctx.http,
            serenity::all::CreateInteractionResponse::Autocomplete(response),
        )
        .await?;
    Ok(())
}

/// Installs the `LDraw` part library and exits.
///
/// Split out of `main` because it is a self-contained one-shot: it needs
/// neither the database nor a Discord token.
async fn install_ldraw_library(config: &Config) -> ! {
    let http_client = crate::http::HttpClient::new();
    match crate::ldraw_sync::sync(&http_client, &config.ldraw).await {
        Ok(report) => {
            tracing::info!(
                "Installed {} LDraw files ({} bytes) into {}; skipped {} non-redistributable file(s).",
                report.files_written,
                report.bytes_written,
                config.ldraw.data_dir,
                report.skipped_non_redistributable
            );
            std::process::exit(0);
        }
        Err(e) => {
            tracing::error!("Failed to install the LDraw library: {e}");
            std::process::exit(1);
        }
    }
}

/// Opens the `--build-debug-log` file, if one was requested.
///
/// A bad path is a startup misconfiguration, not a reason to run without the
/// trace the operator explicitly asked for, so it exits rather than warns.
async fn open_build_debug_log(
    path: Option<&str>,
) -> Option<std::sync::Arc<crate::build::debug_log::BuildDebugLog>> {
    let path = path?;
    match crate::build::debug_log::BuildDebugLog::open(path).await {
        Ok(log) => {
            tracing::info!("Logging /build debug trace to {}", path);
            Some(std::sync::Arc::new(log))
        }
        Err(e) => {
            tracing::error!("Failed to open --build-debug-log {}: {}", path, e);
            std::process::exit(1);
        }
    }
}

/// Opens the sticky part cache, or explains why there is none.
///
/// A cache that will not open is a degradation, not a failure: autocomplete
/// falls back to the built-in catalogue and every lookup goes to the network
/// as it does today. Saying so once at startup is what stops that being a
/// mystery later.
async fn open_parts_cache(
    config: &Config,
) -> Option<std::sync::Arc<crate::parts_cache::PartCache>> {
    if !config.parts_cache.enabled {
        tracing::info!("Part cache disabled; autocomplete will use the built-in catalogue only.");
        return None;
    }
    let ttl = crate::parts_cache::Ttl {
        search_days: config.parts_cache.search_ttl_days,
        miss_mins: config.parts_cache.miss_ttl_mins,
    };
    match crate::parts_cache::PartCache::open(&config.parts_cache.path, ttl).await {
        Ok(cache) => {
            let known = cache.len().await.unwrap_or(0);
            tracing::info!(
                "Part cache open at {} ({known} part(s) known).",
                config.parts_cache.path
            );
            Some(std::sync::Arc::new(cache))
        }
        Err(e) => {
            tracing::error!(
                "Could not open the part cache at {}: {e}. Falling back to the built-in \
                 catalogue; part lookups will not be remembered.",
                config.parts_cache.path
            );
            None
        }
    }
}

/// Drops expired searches from the part cache once a day.
///
/// Only searches expire. The parts themselves are the index that makes
/// autocomplete instant and are never swept — that is the whole point of a
/// cache that gets better the longer it runs.
fn spawn_part_cache_sweep(cache: Option<std::sync::Arc<crate::parts_cache::PartCache>>) {
    let Some(cache) = cache else {
        return;
    };
    tokio::spawn(async move {
        loop {
            if let Err(e) = cache.sweep().await {
                tracing::error!("Failed to sweep the part cache: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_hours(24)).await;
        }
    });
}

async fn setup_database(database_url: &str) -> anyhow::Result<SqlitePool> {
    let connect_options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await?;

    tracing::info!("Running database migrations...");
    sqlx::migrate!().run(&pool).await?;
    tracing::info!("Migrations completed successfully.");

    Ok(pool)
}

fn start_polling_tasks(
    config: &std::sync::Arc<Config>,
    pool: &SqlitePool,
    http: &std::sync::Arc<serenity::all::Http>,
    _data: std::sync::Arc<tokio::sync::RwLock<serenity::prelude::TypeMap>>,
    sync_mutex: &std::sync::Arc<tokio::sync::Mutex<()>>,
) {
    // Start global RSS polling loop
    {
        let db_clone = pool.clone();
        let http_clone = http.clone();
        let config_clone = config.clone();
        tokio::spawn(async move {
            crate::rss::global_poll_loop(http_clone, db_clone, config_clone).await;
        });
    }

    let (pb_tx, _pb_rx) = tokio::sync::broadcast::channel::<crate::events_sync::SyncMessage>(100);

    for guild in config.guilds.clone() {
        let guild_id = GuildId::new(guild.guild_id);

        if config.is_sync_events_enabled() {
            let db_clone = pool.clone();
            let discord_http = http.clone();

            if let Some(pb_cfg) = &config.pocketbase {
                let pb_cfg = pb_cfg.clone();
                let http_client = std::sync::Arc::new(crate::http::HttpClient::new());
                let limit = config.resource_limits.max_http_body_bytes;
                let rx = pb_tx.subscribe();
                let sync_mutex = sync_mutex.clone();
                tokio::spawn(async move {
                    crate::events_sync::discord_sync_worker(
                        discord_http,
                        http_client,
                        db_clone,
                        guild_id,
                        pb_cfg,
                        limit,
                        rx,
                        sync_mutex,
                    )
                    .await;
                });
            } else {
                tracing::error!(
                    "PocketBase is not configured, cannot sync events for guild {}",
                    guild_id
                );
            }
        }
    }

    if config.is_sync_events_enabled() {
        if let Some(pb_cfg) = &config.pocketbase {
            let pb_cfg = pb_cfg.clone();
            let http_client = std::sync::Arc::new(crate::http::HttpClient::new());
            let limit = config.resource_limits.max_http_body_bytes;
            let tx = pb_tx;
            tokio::spawn(async move {
                crate::events_sync::pocketbase_source_loop(http_client, pb_cfg, limit, tx).await;
            });
        }
    }
}

/// Housekeeping that runs for the life of the process: the daily sweep of
/// expired rows and the events outbox worker.
fn spawn_maintenance_tasks(pool: &SqlitePool, config: &Config) {
    // Ambient cleanup
    {
        let pool = pool.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = crate::db::cleanup_expired_rows(&pool).await {
                    tracing::error!("Failed to cleanup expired rows: {}", e);
                } else {
                    tracing::debug!("Successfully cleaned up expired rows.");
                }
                // Once a day.
                tokio::time::sleep(std::time::Duration::from_hours(24)).await;
            }
        });
    }

    // Outbox worker
    {
        let pool = pool.clone();
        let config_clone = std::sync::Arc::new(config.clone());
        let http_client: std::sync::Arc<dyn crate::http::HttpProvider> =
            std::sync::Arc::new(crate::http::HttpClient::new());
        tokio::spawn(async move {
            crate::workflows::events::outbox_worker(pool, config_clone, http_client).await;
        });
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    let config = match Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load config from {}: {}", cli.config, e);
            std::process::exit(1);
        }
    };

    if cli.check_config {
        tracing::info!("Configuration in {} is valid.", cli.config);
        std::process::exit(0);
    }

    // Installing the part library needs neither a database nor a Discord
    // token, so it runs before either is set up.
    if cli.fetch_ldraw {
        install_ldraw_library(&config).await;
    }

    // Installed before anything slow, so a `systemctl reload` during startup
    // is queued rather than killing the process: SIGHUP terminates by default.
    let reload_requests = crate::reload::spawn_sighup_listener()?;

    let database_url =
        env::var("DATABASE_URL").expect("Expected a database url in the environment");

    let pool = setup_database(&database_url).await?;

    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    let build_debug_log = open_build_debug_log(cli.build_debug_log.as_deref()).await;

    crate::http::init_cache(config.cache_ttl_secs.unwrap_or(600), 50 * 1024 * 1024);

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::DIRECT_MESSAGES;

    spawn_maintenance_tasks(&pool, &config);

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler {
            config: config.clone(),
            pool: pool.clone(),
            user_cooldowns: moka::future::Cache::builder()
                .time_to_idle(std::time::Duration::from_hours(2))
                .build(),
            build_debug_log,
        })
        .await?;

    let http_client: std::sync::Arc<dyn crate::http::HttpProvider> =
        std::sync::Arc::new(crate::http::HttpClient::new());

    let sync_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));

    let parts_cache = open_parts_cache(&config).await;

    {
        let mut data = client.data.write().await;
        data.insert::<ConfigData>(std::sync::Arc::new(config.clone()));
        data.insert::<DbData>(pool.clone());
        data.insert::<HttpData>(http_client.clone());
        data.insert::<SyncMutexData>(sync_mutex.clone());
        data.insert::<PartsCacheData>(parts_cache.clone());
        // One library for the whole process, so its parsed-file and mesh
        // caches are shared rather than rebuilt per render.
        data.insert::<LdrawData>(std::sync::Arc::new(
            crate::ldraw::resolve::LdrawLibrary::new(
                &config.ldraw.data_dir,
                crate::ldraw::resolve::ResolveLimits {
                    max_files: config.ldraw.max_files,
                    ..crate::ldraw::resolve::ResolveLimits::default()
                },
            ),
        ));
    }

    spawn_part_cache_sweep(parts_cache.clone());

    let http = client.http.clone();
    let data = client.data.clone();

    let config_arc = std::sync::Arc::new(config.clone());
    start_polling_tasks(&config_arc, &pool, &http, data, &sync_mutex);

    let webhook_state = crate::zulip::AppState {
        db: pool.clone(),
        config: std::sync::Arc::new(config.clone()),
        http: http_client,
        discord: client.http.clone(),
    };

    // Removing `await` if `start_event_listener` is no longer async
    let () = crate::zulip::start_event_listener(webhook_state);

    let shard_manager = client.shard_manager.clone();
    let config_path = cli.config.clone();

    tokio::select! {
        result = client.start() => {
            if let Err(why) = result {
                tracing::error!("Client error: {why:?}");
            }
        }
        _ = crate::reload::wait_for_valid_reload(reload_requests, || Config::load(&config_path)) => {
            tracing::info!("Configuration in {} is valid, restarting in place.", cli.config);
            restart_in_place(&shard_manager, &pool, parts_cache.as_deref()).await;
        }
    }

    Ok(())
}

/// Longest any one shutdown step may hold up a reload.
const RELOAD_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Closes the gateway and the databases, then replaces this process with a
/// fresh copy of itself, which reads the new configuration on its way up.
/// The PID is kept, so a service manager sees one uninterrupted service.
async fn restart_in_place(
    shard_manager: &serenity::gateway::ShardManager,
    pool: &SqlitePool,
    parts_cache: Option<&crate::parts_cache::PartCache>,
) -> ! {
    // Background tasks are still running and may hold a connection; none of
    // them is allowed to stall the reload.
    if tokio::time::timeout(RELOAD_SHUTDOWN_TIMEOUT, shard_manager.shutdown_all())
        .await
        .is_err()
    {
        tracing::warn!("Timed out closing the Discord gateway before reload.");
    }
    if tokio::time::timeout(RELOAD_SHUTDOWN_TIMEOUT, pool.close())
        .await
        .is_err()
    {
        tracing::warn!("Timed out closing the database before reload.");
    }
    if let Some(cache) = parts_cache {
        if tokio::time::timeout(RELOAD_SHUTDOWN_TIMEOUT, cache.close())
            .await
            .is_err()
        {
            tracing::warn!("Timed out closing the part cache before reload.");
        }
    }

    reexec()
}

/// Replaces the process image with this program's own command line. Exits
/// non-zero if that fails, leaving the service manager to start it again.
fn reexec() -> ! {
    use std::os::unix::process::CommandExt;

    let fallback = env::current_exe().unwrap_or_else(|_| "brickbot".into());
    let (program, args) = crate::reload::exec_target(env::args_os(), fallback);
    // `exec` returns only if it failed.
    let err = std::process::Command::new(&program).args(args).exec();
    tracing::error!("Failed to re-execute {}: {err}", program.to_string_lossy());
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, GuildConfig};

    #[test]
    fn test_is_guild_allowed() {
        let config = Config {
            guilds: vec![
                GuildConfig {
                    guild_id: 12345,
                    locale: None,
                    bot_name: None,
                    ambient_channel_ids: None,
                    interactions: None,
                    commands: None,
                },
                GuildConfig {
                    guild_id: 67890,
                    locale: None,
                    bot_name: None,
                    ambient_channel_ids: None,
                    interactions: None,
                    commands: None,
                },
            ],
            ..Default::default()
        };

        assert!(is_guild_allowed(&config, 12345));
        assert!(is_guild_allowed(&config, 67890));
        assert!(!is_guild_allowed(&config, 11111));
    }
}
