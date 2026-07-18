use serde::Deserialize;
use std::fs;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Config {
    pub locale: Option<String>,
    pub bot_name: Option<String>,
    #[serde(default)]
    pub ambient_debug_logging: bool,
    pub ambient_training_data: Option<AmbientTrainingDataConfig>,
    pub zulip: Option<ZulipConfig>,
    pub pocketbase: Option<PocketBaseConfig>,
    pub poll_interval: Option<u64>,
    pub cache_ttl_secs: Option<u64>,
    #[serde(default)]
    pub interactions: InteractionsConfig,
    #[serde(default)]
    pub commands: CommandsConfig,
    #[serde(default)]
    pub guilds: Vec<GuildConfig>,
    #[serde(default)]
    pub feeds: Vec<RssFeedConfig>,
    #[serde(default)]
    pub resource_limits: ResourceLimitsConfig,
}

#[derive(Deserialize, Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct InteractionsConfig {
    #[serde(default = "default_true")]
    pub set: bool,
    #[serde(default = "default_true")]
    pub part: bool,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct GuildInteractionsConfig {
    pub set: Option<bool>,
    pub part: Option<bool>,
}

impl Default for InteractionsConfig {
    fn default() -> Self {
        Self {
            set: true,
            part: true,
        }
    }
}

const fn default_true() -> bool {
    true
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CommandsConfig {
    #[serde(default)]
    pub events: EventsCommandConfig,
    #[serde(default)]
    pub set: SetCommandConfig,
    #[serde(default)]
    pub part: PartCommandConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsCommandConfig,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct GuildCommandsConfig {
    pub events: Option<EventsCommandConfig>,
    pub set: Option<SetCommandConfig>,
    pub part: Option<PartCommandConfig>,
    pub diagnostics: Option<DiagnosticsCommandConfig>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct EventsCommandConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub enable_edit: bool,
    #[serde(default)]
    pub enable_propose: bool,
    #[serde(default = "default_true")]
    pub enable_fallback_mention: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SetCommandConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for SetCommandConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct PartCommandConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for PartCommandConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct DiagnosticsCommandConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GuildConfig {
    pub guild_id: u64,
    pub locale: Option<String>,
    pub bot_name: Option<String>,
    #[serde(default)]
    pub ambient_channel_ids: Option<Vec<u64>>,
    pub interactions: Option<GuildInteractionsConfig>,
    pub commands: Option<GuildCommandsConfig>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RssFeedConfig {
    #[serde(default)]
    pub feed_urls: Vec<String>,
    #[serde(default)]
    pub opml_urls: Vec<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct AmbientTrainingDataConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    pub allowed_guild_ids: Option<Vec<u64>>,
    #[serde(default)]
    pub store_raw_text: bool,
}

const fn default_retention_days() -> u32 {
    30
}

#[derive(Deserialize, Debug, Clone)]
pub struct ZulipConfig {
    pub url: String,
    pub bot_email: String,
    pub moderation_stream: String,
    #[serde(default)]
    pub moderators: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PocketBaseConfig {
    pub url: String,
    pub collection: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ResourceLimitsConfig {
    #[serde(default = "default_max_http_body_bytes")]
    pub max_http_body_bytes: u64,
    #[serde(default = "default_max_cache_memory_bytes")]
    pub max_cache_memory_bytes: u64,
    #[serde(default = "default_max_feed_items_per_sync")]
    pub max_feed_items_per_sync: usize,
    #[serde(default = "default_max_worker_concurrency")]
    pub max_worker_concurrency: usize,
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self {
            max_http_body_bytes: default_max_http_body_bytes(),
            max_cache_memory_bytes: default_max_cache_memory_bytes(),
            max_feed_items_per_sync: default_max_feed_items_per_sync(),
            max_worker_concurrency: default_max_worker_concurrency(),
        }
    }
}

const fn default_max_http_body_bytes() -> u64 {
    10 * 1024 * 1024 // 10 MB
}

const fn default_max_cache_memory_bytes() -> u64 {
    50 * 1024 * 1024 // 50 MB
}

const fn default_max_feed_items_per_sync() -> usize {
    1000
}

const fn default_max_worker_concurrency() -> usize {
    4
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        config.validate().map_err(|errs| {
            anyhow::anyhow!(
                "Configuration validation failed:\n  - {}",
                errs.join("\n  - ")
            )
        })?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        let mut guild_ids = std::collections::HashSet::new();
        for guild in &self.guilds {
            if guild.guild_id == 0 {
                errors.push("Guild ID cannot be 0".to_string());
            }
            if !guild_ids.insert(guild.guild_id) {
                errors.push(format!("Duplicate guild ID: {}", guild.guild_id));
            }
        }

        if let Some(pb) = &self.pocketbase {
            if !pb.url.starts_with("http://") && !pb.url.starts_with("https://") {
                errors.push(format!(
                    "PocketBase URL must start with http:// or https://: {}",
                    pb.url
                ));
            }
            if pb.collection.is_empty() {
                errors.push("PocketBase collection must not be empty".to_string());
            }
        }

        if let Some(z) = &self.zulip {
            if !z.url.starts_with("http://") && !z.url.starts_with("https://") {
                errors.push(format!(
                    "Zulip URL must start with http:// or https://: {}",
                    z.url
                ));
            }
            if !z.bot_email.contains('@') {
                errors.push(format!("Zulip bot email is invalid: {}", z.bot_email));
            }
        }

        for feed in &self.feeds {
            for url in &feed.feed_urls {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    errors.push(format!(
                        "Feed URL must start with http:// or https://: {url}"
                    ));
                }
            }
            for url in &feed.opml_urls {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    errors.push(format!(
                        "OPML URL must start with http:// or https://: {url}"
                    ));
                }
            }
        }

        if let Some(interval) = self.poll_interval {
            if interval > 0 && interval < 10 {
                errors.push("poll_interval must be at least 10 seconds".to_string());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    #[must_use]
    pub const fn is_sync_events_enabled(&self) -> bool {
        self.pocketbase.is_some()
    }

    #[must_use]
    pub fn get_guild_config(&self, guild_id: u64) -> Option<&GuildConfig> {
        self.guilds.iter().find(|g| g.guild_id == guild_id)
    }

    #[must_use]
    pub fn interactions_for(&self, guild_id: u64) -> InteractionsConfig {
        let g = self
            .get_guild_config(guild_id)
            .and_then(|g| g.interactions.as_ref());
        InteractionsConfig {
            set: g.and_then(|c| c.set).unwrap_or(self.interactions.set),
            part: g.and_then(|c| c.part).unwrap_or(self.interactions.part),
        }
    }

    #[must_use]
    pub fn commands_for(&self, guild_id: u64) -> CommandsConfig {
        let g = self
            .get_guild_config(guild_id)
            .and_then(|g| g.commands.as_ref());
        CommandsConfig {
            events: g
                .and_then(|c| c.events.clone())
                .unwrap_or_else(|| self.commands.events.clone()),
            set: g
                .and_then(|c| c.set.clone())
                .unwrap_or_else(|| self.commands.set.clone()),
            part: g
                .and_then(|c| c.part.clone())
                .unwrap_or_else(|| self.commands.part.clone()),
            diagnostics: g
                .and_then(|c| c.diagnostics.clone())
                .unwrap_or_else(|| self.commands.diagnostics.clone()),
        }
    }

    #[must_use]
    pub fn locale_for(&self, guild_id: u64) -> Option<&str> {
        self.get_guild_config(guild_id)
            .and_then(|g| g.locale.as_deref())
            .or(self.locale.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_parsing() {
        let toml_str = r#"
        locale = "fi-FI"
        bot_name = "Global Bot"
        poll_interval = 300

        [[guilds]]
        guild_id = 123
        bot_name = "Server Bot"
        locale = "fi-FI"
        ambient_channel_ids = [300]

        [guilds.interactions]
        set = false

        [guilds.commands.events]
        enabled = false

        [[feeds]]
        feed_urls = ["http://example.com/rss"]

        [interactions]
        set = true
        part = false

        [commands.events]
        enabled = true
        enable_edit = true
        enable_propose = false

        [commands.set]
        enabled = false

        [commands.diagnostics]
        enabled = true
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.locale.as_deref(), Some("fi-FI"));
        assert_eq!(config.bot_name.as_deref(), Some("Global Bot"));
        assert_eq!(config.guilds.len(), 1);

        let guild = &config.guilds[0];
        assert_eq!(guild.guild_id, 123);
        assert_eq!(guild.bot_name.as_deref(), Some("Server Bot"));
        assert_eq!(guild.locale.as_deref(), Some("fi-FI"));
        assert_eq!(guild.ambient_channel_ids.as_ref().unwrap(), &vec![300]);
        assert_eq!(config.feeds.len(), 1);
        assert_eq!(config.feeds[0].feed_urls[0], "http://example.com/rss");
        assert_eq!(config.poll_interval, Some(300));
        assert!(config.interactions.set);
        assert!(!config.interactions.part);
        assert!(config.commands.events.enabled);
        assert!(config.commands.events.enable_edit);
        assert!(!config.commands.events.enable_propose);
        assert!(config.commands.events.enable_fallback_mention);
        assert!(!config.commands.set.enabled);
        assert!(config.commands.diagnostics.enabled);

        assert!(!config.interactions_for(123).set);
        assert!(!config.commands_for(123).events.enabled);
        assert!(config.interactions_for(999).set);
        assert!(config.commands_for(999).events.enabled);

        assert_eq!(config.resource_limits.max_http_body_bytes, 10 * 1024 * 1024);
        assert_eq!(
            config.resource_limits.max_cache_memory_bytes,
            50 * 1024 * 1024
        );
        assert_eq!(config.resource_limits.max_feed_items_per_sync, 1000);
        assert_eq!(config.resource_limits.max_worker_concurrency, 4);
    }
}
