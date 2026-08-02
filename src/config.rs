use serde::Deserialize;
use std::fs;

/// The locale used when neither a guild nor the global config names one.
///
/// The bot is built for a Finnish community, so an unconfigured deployment
/// speaks Finnish. This is the single fallback: nothing else may invent one,
/// or command names and replies drift into different languages.
pub const DEFAULT_LOCALE: &str = "fi-FI";

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
    #[serde(default)]
    pub ldraw: LdrawConfig,
    #[serde(default)]
    pub parts_cache: PartsCacheConfig,
}

/// The sticky, on-disk cache of part lookups.
///
/// Discord fires autocomplete on every keystroke with a three-second deadline,
/// so the network can never be on that path. Instead every lookup the bot
/// performs is kept, building a local index that answers instantly and
/// survives restarts. It lives in its own database because all of it is
/// derived and disposable — deleting the file is a clean rebuild.
#[derive(Deserialize, Debug, Clone)]
pub struct PartsCacheConfig {
    /// With this off, autocomplete falls back to the built-in catalogue alone.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Deliberately not `bot.db`: that holds sessions and revisions, which
    /// cannot be regenerated.
    #[serde(default = "default_parts_cache_path")]
    pub path: String,
    /// How long a search that found something stays good. Rebrickable does add
    /// parts, so this is long rather than forever.
    #[serde(default = "default_parts_cache_search_ttl_days")]
    pub search_ttl_days: i64,
    /// How long a search that found *nothing* stays good. Short, so a typo is
    /// not pinned — but not zero, or every keystroke would re-ask.
    #[serde(default = "default_parts_cache_miss_ttl_mins")]
    pub miss_ttl_mins: i64,
    /// How many background warms may run at once.
    #[serde(default = "default_parts_cache_warm_concurrency")]
    pub warm_concurrency: usize,
    /// Gives up on a warm rather than holding a connection open. `src/http.rs`
    /// has no timeout of its own.
    #[serde(default = "default_parts_cache_warm_timeout_secs")]
    pub warm_timeout_secs: u64,
}

impl Default for PartsCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_parts_cache_path(),
            search_ttl_days: default_parts_cache_search_ttl_days(),
            miss_ttl_mins: default_parts_cache_miss_ttl_mins(),
            warm_concurrency: default_parts_cache_warm_concurrency(),
            warm_timeout_secs: default_parts_cache_warm_timeout_secs(),
        }
    }
}

fn default_parts_cache_path() -> String {
    "sqlite:data/cache.db".to_string()
}

const fn default_parts_cache_search_ttl_days() -> i64 {
    30
}

const fn default_parts_cache_miss_ttl_mins() -> i64 {
    60
}

const fn default_parts_cache_warm_concurrency() -> usize {
    2
}

const fn default_parts_cache_warm_timeout_secs() -> u64 {
    5
}

/// Where the `LDraw` part library lives and where to fetch it from.
///
/// The library is never embedded in the binary and never downloaded during a
/// render: an operator installs it once with `brickbot --fetch-ldraw`, and
/// renders read it from disk. Until then the bot renders in box mode.
#[derive(Deserialize, Debug, Clone)]
pub struct LdrawConfig {
    /// Directory holding the installed library.
    #[serde(default = "default_ldraw_data_dir")]
    pub data_dir: String,
    /// Archive `--fetch-ldraw` downloads.
    #[serde(default = "default_ldraw_library_url")]
    pub library_url: String,
    /// Cap on the number of files read while resolving a single part.
    #[serde(default = "default_ldraw_max_files")]
    pub max_files: usize,
    /// Size limit for the library download. Far larger than an ordinary API
    /// response, so it is separate from `resource_limits`.
    #[serde(default = "default_ldraw_max_download_bytes")]
    pub max_download_bytes: u64,
}

impl Default for LdrawConfig {
    fn default() -> Self {
        Self {
            data_dir: default_ldraw_data_dir(),
            library_url: default_ldraw_library_url(),
            max_files: default_ldraw_max_files(),
            max_download_bytes: default_ldraw_max_download_bytes(),
        }
    }
}

fn default_ldraw_data_dir() -> String {
    "data/ldraw".to_string()
}

fn default_ldraw_library_url() -> String {
    "https://library.ldraw.org/library/updates/complete.zip".to_string()
}

const fn default_ldraw_max_files() -> usize {
    200
}

const fn default_ldraw_max_download_bytes() -> u64 {
    256 * 1024 * 1024 // 256 MB; the official archive is well under this.
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
    pub locations: LocationsCommandConfig,
    #[serde(default)]
    pub set: SetCommandConfig,
    #[serde(default)]
    pub part: PartCommandConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsCommandConfig,
    #[serde(default)]
    pub build: BuildCommandConfig,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct GuildCommandsConfig {
    pub events: Option<EventsCommandConfig>,
    pub locations: Option<LocationsCommandConfig>,
    pub set: Option<SetCommandConfig>,
    pub part: Option<PartCommandConfig>,
    pub diagnostics: Option<DiagnosticsCommandConfig>,
    pub build: Option<BuildCommandConfig>,
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

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct LocationsCommandConfig {
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

/// Settings for the collaborative `/build` command.
#[derive(Deserialize, Debug, Clone)]
pub struct BuildCommandConfig {
    /// Off by default, like `events` and `diagnostics`: it opens threads and
    /// listens to chat, so it should be switched on deliberately.
    #[serde(default)]
    pub enabled: bool,
    /// How long a session lives without activity.
    #[serde(default = "default_build_session_ttl_hours")]
    pub session_ttl_hours: u32,
    #[serde(default = "default_build_render_width")]
    pub render_width: u32,
    #[serde(default = "default_build_render_height")]
    pub render_height: u32,
    /// Render at this multiple and downsample, for cheap anti-aliasing.
    #[serde(default = "default_build_supersample")]
    pub supersample: u32,
    /// A burst of commands coalesces into one render after this long, so four
    /// people typing at once produce one picture rather than four.
    #[serde(default = "default_build_render_debounce_ms")]
    pub render_debounce_ms: u64,
    /// Turn off if `BrickLink` Studio rejects our `.io` container; the export
    /// then falls back to a plain `.ldr`, which Studio imports natively.
    #[serde(default = "default_true")]
    pub enable_io_export: bool,
    /// The largest a model may grow, when a session does not give its own
    /// `size` at creation: a cube of this many studs on X/Z and bricks on Y.
    #[serde(default = "default_build_max_model_size")]
    pub max_model_size: u32,
}

impl Default for BuildCommandConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            session_ttl_hours: default_build_session_ttl_hours(),
            render_width: default_build_render_width(),
            render_height: default_build_render_height(),
            supersample: default_build_supersample(),
            render_debounce_ms: default_build_render_debounce_ms(),
            enable_io_export: true,
            max_model_size: default_build_max_model_size(),
        }
    }
}

const fn default_build_session_ttl_hours() -> u32 {
    24
}

const fn default_build_render_width() -> u32 {
    800
}

const fn default_build_render_height() -> u32 {
    600
}

const fn default_build_supersample() -> u32 {
    2
}

const fn default_build_render_debounce_ms() -> u64 {
    1200
}

const fn default_build_max_model_size() -> u32 {
    10
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
    /// Collection holding map locations. Defaults to `locations` when unset.
    #[serde(default)]
    pub locations_collection: Option<String>,
}

impl PocketBaseConfig {
    /// Name of the collection holding map locations.
    #[must_use]
    pub fn locations_collection(&self) -> &str {
        self.locations_collection
            .as_deref()
            .unwrap_or(DEFAULT_LOCATIONS_COLLECTION)
    }
}

const DEFAULT_LOCATIONS_COLLECTION: &str = "locations";

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
            if pb
                .locations_collection
                .as_ref()
                .is_some_and(String::is_empty)
            {
                errors.push("PocketBase locations_collection must not be empty".to_string());
            }
        }

        if !self.ldraw.library_url.starts_with("http://")
            && !self.ldraw.library_url.starts_with("https://")
        {
            errors.push(format!(
                "LDraw library URL must start with http:// or https://: {}",
                self.ldraw.library_url
            ));
        }
        if self.ldraw.data_dir.trim().is_empty() {
            errors.push("LDraw data_dir must not be empty".to_string());
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

        if self.commands.build.max_model_size == 0 {
            errors.push("commands.build.max_model_size must be at least 1".to_string());
        }
        for guild in &self.guilds {
            if let Some(build) = guild.commands.as_ref().and_then(|c| c.build.as_ref()) {
                if build.max_model_size == 0 {
                    errors.push(format!(
                        "guild {} commands.build.max_model_size must be at least 1",
                        guild.guild_id
                    ));
                }
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
            locations: g
                .and_then(|c| c.locations.clone())
                .unwrap_or_else(|| self.commands.locations.clone()),
            set: g
                .and_then(|c| c.set.clone())
                .unwrap_or_else(|| self.commands.set.clone()),
            part: g
                .and_then(|c| c.part.clone())
                .unwrap_or_else(|| self.commands.part.clone()),
            diagnostics: g
                .and_then(|c| c.diagnostics.clone())
                .unwrap_or_else(|| self.commands.diagnostics.clone()),
            build: g
                .and_then(|c| c.build.clone())
                .unwrap_or_else(|| self.commands.build.clone()),
        }
    }

    /// The locale configured for a guild, if any.
    ///
    /// Prefer [`Self::locale_or_default`] when a locale is actually needed:
    /// this returns `None` for an unconfigured deployment, and every call site
    /// picking its own fallback is how the bot ended up registering `/rakenna`
    /// and then answering in English.
    #[must_use]
    pub fn locale_for(&self, guild_id: u64) -> Option<&str> {
        self.get_guild_config(guild_id)
            .and_then(|g| g.locale.as_deref())
            .or(self.locale.as_deref())
    }

    /// The locale to use for a guild, falling back to [`DEFAULT_LOCALE`].
    #[must_use]
    pub fn locale_or_default(&self, guild_id: u64) -> &str {
        self.locale_for(guild_id).unwrap_or(DEFAULT_LOCALE)
    }

    /// The locale to use outside any guild — a DM, or a Zulip topic whose
    /// Discord channel could not be resolved.
    #[must_use]
    pub fn default_locale(&self) -> &str {
        self.locale.as_deref().unwrap_or(DEFAULT_LOCALE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unconfigured_deployment_speaks_the_default_locale() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.locale_for(123), None);
        assert_eq!(config.locale_or_default(123), DEFAULT_LOCALE);
        assert_eq!(config.default_locale(), DEFAULT_LOCALE);
    }

    #[test]
    fn a_guild_locale_wins_over_the_global_one_which_wins_over_the_default() {
        let config: Config = toml::from_str(
            r#"
            locale = "en-US"

            [[guilds]]
            guild_id = 1
            locale = "fi-FI"

            [[guilds]]
            guild_id = 2
            "#,
        )
        .unwrap();

        assert_eq!(config.locale_or_default(1), "fi-FI");
        // No guild locale: the global one applies.
        assert_eq!(config.locale_or_default(2), "en-US");
        // Not a configured guild at all: still the global one.
        assert_eq!(config.locale_or_default(999), "en-US");
        assert_eq!(config.default_locale(), "en-US");
    }

    #[test]
    fn build_command_config_defaults_are_sane() {
        let config: Config = toml::from_str("").unwrap();
        let build = &config.commands.build;
        // Off unless switched on deliberately, like events and diagnostics.
        assert!(!build.enabled);
        assert!(build.session_ttl_hours > 0);
        assert!(build.render_width >= 100 && build.render_height >= 100);
        assert!(build.supersample >= 1);
        assert!(build.render_debounce_ms > 0);
        assert!(build.enable_io_export);
        assert!(build.max_model_size > 0);
    }

    #[test]
    fn build_command_can_be_enabled_and_overridden_per_guild() {
        let toml_str = r"
        [commands.build]
        enabled = true
        render_width = 1200

        [[guilds]]
        guild_id = 123

        [guilds.commands.build]
        enabled = false
        ";
        let config: Config = toml::from_str(toml_str).unwrap();

        assert!(config.commands.build.enabled);
        assert_eq!(config.commands.build.render_width, 1200);
        // Unset keys still take their defaults.
        assert_eq!(config.commands.build.render_height, 600);

        // The guild turns it back off.
        assert!(!config.commands_for(123).build.enabled);
        // An unlisted guild inherits the global setting.
        assert!(config.commands_for(999).build.enabled);
    }

    #[test]
    fn ldraw_config_defaults_are_sane() {
        // An empty config still yields a usable LDraw section, so an operator
        // only has to set what they want to change.
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.ldraw.data_dir, "data/ldraw");
        assert!(config.ldraw.library_url.starts_with("https://"));
        assert!(config.ldraw.max_files > 0);
        assert!(config.ldraw.max_download_bytes > 1024 * 1024);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn ldraw_config_can_be_overridden() {
        let toml_str = r#"
        [ldraw]
        data_dir = "/srv/ldraw"
        library_url = "https://mirror.example.org/complete.zip"
        max_files = 50
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.ldraw.data_dir, "/srv/ldraw");
        assert_eq!(config.ldraw.max_files, 50);
        // Unset keys still take their defaults.
        assert!(config.ldraw.max_download_bytes > 0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn ldraw_config_rejects_a_non_http_library_url() {
        let config: Config = toml::from_str(
            r#"
        [ldraw]
        library_url = "file:///etc/passwd"
        "#,
        )
        .unwrap();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("LDraw library URL")));
    }

    #[test]
    fn ldraw_config_rejects_an_empty_data_dir() {
        let config: Config = toml::from_str(
            r#"
        [ldraw]
        data_dir = "  "
        "#,
        )
        .unwrap();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("data_dir")));
    }

    #[test]
    fn a_zero_max_model_size_is_rejected() {
        let config: Config = toml::from_str(
            r"
        [commands.build]
        max_model_size = 0
        ",
        )
        .unwrap();
        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("max_model_size")));
    }

    #[test]
    fn a_zero_max_model_size_in_a_guild_override_is_rejected() {
        let config: Config = toml::from_str(
            r"
        [[guilds]]
        guild_id = 1
        [guilds.commands.build]
        max_model_size = 0
        ",
        )
        .unwrap();
        let errors = config.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("max_model_size") && e.contains('1'))
        );
    }

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

        [guilds.commands.locations]
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

        [commands.locations]
        enabled = true
        enable_edit = true
        enable_propose = true

        [commands.set]
        enabled = false

        [commands.diagnostics]
        enabled = true

        [pocketbase]
        url = "https://pb.example.com"
        collection = "events"
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
        assert!(config.commands.locations.enabled);
        assert!(config.commands.locations.enable_edit);
        assert!(config.commands.locations.enable_propose);
        assert!(config.commands.locations.enable_fallback_mention);

        assert!(!config.interactions_for(123).set);
        assert!(!config.commands_for(123).events.enabled);
        assert!(!config.commands_for(123).locations.enabled);
        assert!(config.interactions_for(999).set);
        assert!(config.commands_for(999).events.enabled);
        assert!(config.commands_for(999).locations.enabled);

        // `locations_collection` is optional and falls back to the default name.
        let pb = config.pocketbase.as_ref().unwrap();
        assert_eq!(pb.collection, "events");
        assert_eq!(pb.locations_collection(), "locations");

        assert_eq!(config.resource_limits.max_http_body_bytes, 10 * 1024 * 1024);
        assert_eq!(
            config.resource_limits.max_cache_memory_bytes,
            50 * 1024 * 1024
        );
        assert_eq!(config.resource_limits.max_feed_items_per_sync, 1000);
        assert_eq!(config.resource_limits.max_worker_concurrency, 4);
    }

    #[test]
    fn test_locations_collection_override() {
        let toml_str = r#"
        [pocketbase]
        url = "https://pb.example.com"
        collection = "events"
        locations_collection = "map_places"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.pocketbase.as_ref().unwrap().locations_collection(),
            "map_places"
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_empty_locations_collection_is_invalid() {
        let toml_str = r#"
        [pocketbase]
        url = "https://pb.example.com"
        collection = "events"
        locations_collection = ""
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let errors = config.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("locations_collection must not be empty"))
        );
    }
}
