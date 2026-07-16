#![allow(deprecated, unused_mut)]
rust_i18n::i18n!("locales", fallback = "en-US");

mod ambient;
mod brick;
mod commands;
pub mod config;
pub mod db;
pub mod discord_limits;
pub mod events_sync;
pub mod http;
pub mod interactions;
mod links;
mod notifications;
mod pocketbase;
mod rss;
mod workflows;
mod zulip;

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
}

struct Handler {
    config: Config,
    pool: SqlitePool,
    user_cooldowns: moka::future::Cache<String, ()>,
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

pub struct SyncMutexData;
impl TypeMapKey for SyncMutexData {
    type Value = std::sync::Arc<tokio::sync::Mutex<()>>;
}

#[must_use]
pub fn is_guild_allowed(config: &crate::config::Config, guild_id: u64) -> bool {
    config.guilds.iter().any(|g| g.guild_id == guild_id)
}

#[must_use]
pub fn should_forward_help_message(
    guild_config: &crate::config::GuildConfig,
    channel_id: u64,
    parent_channel_id: Option<u64>,
) -> bool {
    if guild_config.help_channel_ids.contains(&channel_id) {
        return true;
    }
    if let Some(parent_id) = parent_channel_id {
        if guild_config.help_forum_channel_ids.contains(&parent_id) {
            return true;
        }
    }
    false
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("{} is connected!", ready.user.name);

        // Clear global commands to ensure we don't have duplicates
        if let Err(e) = Command::set_global_commands(&ctx.http, vec![]).await {
            tracing::error!("Failed to clear global commands: {}", e);
        }

        for guild in &self.config.guilds {
            let mut commands = Vec::new();

            if self.config.commands.events.enabled {
                let locale = guild
                    .locale
                    .as_deref()
                    .or(self.config.locale.as_deref())
                    .unwrap_or("fi-FI");
                commands.push(crate::commands::events::build_events_command(locale));
            }

            if self.config.commands_for(guild.guild_id).set.enabled {
                let locale = guild
                    .locale
                    .as_deref()
                    .or(self.config.locale.as_deref())
                    .unwrap_or("fi-FI");
                commands.push(crate::commands::set::build_set_command(locale));
            }

            if let Err(e) = GuildId::new(guild.guild_id)
                .set_commands(&ctx.http, commands)
                .await
            {
                tracing::error!("Failed to set guild commands for {}: {}", guild.guild_id, e);
            }
        }
    }

    async fn guild_create(&self, ctx: Context, guild: serenity::all::Guild, _is_new: Option<bool>) {
        let guild_id = guild.id.get();

        if !is_guild_allowed(&self.config, guild_id) {
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

        if let Some(guild_id) = msg.guild_id {
            let guild_id_u64 = guild_id.get();
            if let Some(guild_config) = self
                .config
                .guilds
                .iter()
                .find(|g| g.guild_id == guild_id_u64)
            {
                let channel_id_u64 = msg.channel_id.get();
                let mut parent_id = None;

                if !guild_config.help_forum_channel_ids.is_empty() {
                    if let Ok(serenity::all::Channel::Guild(gc)) =
                        msg.channel_id.to_channel(&ctx.http).await
                    {
                        parent_id = gc.parent_id.map(serenity::all::ChannelId::get);
                    }
                }

                if should_forward_help_message(guild_config, channel_id_u64, parent_id) {
                    let zulip_topic = format!("{guild_id_u64}-{channel_id_u64}");
                    let jump_url = msg.link();
                    let author_name = msg.author.name.clone();

                    let content = if msg.content.is_empty() {
                        "<attachment/embed>".to_string()
                    } else {
                        msg.content.clone()
                    };

                    let body =
                        format!("**{author_name}** posted in [Discord]({jump_url}):\n\n{content}");

                    if let Some(zulip_cfg) = &self.config.zulip {
                        if let Some(support_stream) = &zulip_cfg.support_stream {
                            let http_client = std::sync::Arc::new(crate::http::HttpClient::new());
                            let _ = crate::zulip::api::post_topic_to_stream(
                                http_client.as_ref(),
                                zulip_cfg,
                                support_stream,
                                &zulip_topic,
                                &body,
                                self.config.resource_limits.max_http_body_bytes,
                            )
                            .await;
                        }
                    }
                }

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

            let locale = self
                .config
                .locale
                .clone()
                .unwrap_or_else(|| "en-US".to_string());

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

                    let confirmation = rust_i18n::t!(
                        "command.events.dm_reply_sent",
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
                                if let Ok(part) = crate::brick::fetch_part(
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
                        .guilds
                        .iter()
                        .find(|g| g.guild_id == msg.guild_id.unwrap_or_default().get())
                        .and_then(|g| g.locale.clone())
                        .unwrap_or_else(|| "en-US".to_string());

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
            _ => {}
        }
    }
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

    let database_url =
        env::var("DATABASE_URL").expect("Expected a database url in the environment");

    let pool = setup_database(&database_url).await?;

    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    crate::http::init_cache(config.cache_ttl_secs.unwrap_or(600), 50 * 1024 * 1024);

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::DIRECT_MESSAGES;

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
                #[allow(clippy::duration_suboptimal_units)]
                tokio::time::sleep(std::time::Duration::from_secs(86400)).await;
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

    let mut client = Client::builder(&token, intents)
        .event_handler(Handler {
            config: config.clone(),
            pool: pool.clone(),
            user_cooldowns: moka::future::Cache::builder()
                .time_to_idle(std::time::Duration::from_hours(2))
                .build(),
        })
        .await?;

    let http_client: std::sync::Arc<dyn crate::http::HttpProvider> =
        std::sync::Arc::new(crate::http::HttpClient::new());

    let sync_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));

    {
        let mut data = client.data.write().await;
        data.insert::<ConfigData>(std::sync::Arc::new(config.clone()));
        data.insert::<DbData>(pool.clone());
        data.insert::<HttpData>(http_client.clone());
        data.insert::<SyncMutexData>(sync_mutex.clone());
    }

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

    if let Err(why) = client.start().await {
        tracing::error!("Client error: {why:?}");
    }

    Ok(())
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
                    help_channel_ids: vec![],
                    help_forum_channel_ids: vec![],
                    ambient_channel_ids: None,
                    interactions: None,
                    commands: None,
                },
                GuildConfig {
                    guild_id: 67890,
                    locale: None,
                    bot_name: None,
                    help_channel_ids: vec![],
                    help_forum_channel_ids: vec![],
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

    #[test]
    fn test_should_forward_help_message() {
        let guild_config = GuildConfig {
            guild_id: 123,
            locale: None,
            bot_name: None,
            help_channel_ids: vec![100],
            help_forum_channel_ids: vec![200],
            ambient_channel_ids: None,
            interactions: None,
            commands: None,
        };

        assert!(should_forward_help_message(&guild_config, 100, None));
        assert!(!should_forward_help_message(&guild_config, 101, None));

        assert!(should_forward_help_message(&guild_config, 300, Some(200)));
        assert!(!should_forward_help_message(&guild_config, 300, Some(201)));
    }
}
