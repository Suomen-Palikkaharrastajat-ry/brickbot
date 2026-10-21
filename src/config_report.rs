//! `--explain-config`: what a configuration switches on in each guild.

use std::fmt::Write as _;

use crate::config::{Config, GuildConfig, GuildFeedConfig, WorkflowCommandConfig};

/// Where a guild's setting came from: its own `[[guilds]]` entry, or the
/// global settings (defaults included).
const GUILD: &str = "guild";
const GLOBAL: &str = "global";

/// A plain-text report of the global settings and of what is switched on, and
/// how, in every configured guild.
#[must_use]
pub fn explain(config: &Config) -> String {
    let mut out = String::from("Global\n");
    out.push_str(&table(&global_rows(config)));
    if config.guilds.is_empty() {
        out.push_str("\nNo guilds are configured: the bot answers in none.\n");
    }
    for guild in &config.guilds {
        let _ = write!(out, "\nGuild {}\n", guild.guild_id);
        out.push_str(&table(&guild_rows(config, guild)));
    }
    out
}

type Row = [String; 3];

fn row(setting: impl Into<String>, value: impl Into<String>, from: &str) -> Row {
    [setting.into(), value.into(), from.to_string()]
}

const fn from(guild_sets_it: bool) -> &'static str {
    if guild_sets_it { GUILD } else { GLOBAL }
}

const fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn mib(bytes: u64) -> String {
    format!("{} MiB", bytes / (1024 * 1024))
}

fn global_rows(config: &Config) -> Vec<Row> {
    let zulip = config.zulip.as_ref().map_or_else(
        || "off".to_string(),
        |z| {
            format!(
                "on: {}, stream {}",
                without_credentials(&z.url),
                z.moderation_stream
            )
        },
    );
    let pocketbase = config.pocketbase.as_ref().map_or_else(
        || "off".to_string(),
        |pb| {
            format!(
                "on: {}, collections {} and {}",
                without_credentials(&pb.url),
                pb.collection,
                pb.locations_collection()
            )
        },
    );
    let limits = &config.resource_limits;
    let parts_cache = if config.parts_cache.enabled {
        format!("on: {}", config.parts_cache.path)
    } else {
        "off".to_string()
    };
    vec![
        row("Locale", config.default_locale(), GLOBAL),
        row(
            "Bot name",
            config.bot_name.as_deref().unwrap_or("(not set)"),
            GLOBAL,
        ),
        row(
            "Feed polling",
            format!("every {} s", config.feeds.poll_interval_secs()),
            GLOBAL,
        ),
        row("Zulip", zulip, GLOBAL),
        row("PocketBase", pocketbase, GLOBAL),
        row(
            "HTTP cache",
            format!(
                "{} s, {}; bodies up to {}",
                limits.cache_ttl_secs,
                mib(limits.max_cache_memory_bytes),
                mib(limits.max_http_body_bytes)
            ),
            GLOBAL,
        ),
        row("Parts cache", parts_cache, GLOBAL),
        row("LDraw library", config.ldraw.data_dir.clone(), GLOBAL),
    ]
}

fn workflow(command: &WorkflowCommandConfig) -> String {
    if !command.enabled {
        return "off".to_string();
    }
    let options: Vec<&str> = [
        (command.enable_edit, "edit"),
        (command.enable_propose, "propose"),
        (command.enable_fallback_mention, "fallback mention"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    if options.is_empty() {
        "on".to_string()
    } else {
        format!("on: {}", options.join(", "))
    }
}

fn guild_rows(config: &Config, guild: &GuildConfig) -> Vec<Row> {
    let id = guild.guild_id;
    let mut rows = vec![
        row(
            "Locale",
            config.locale_or_default(id),
            from(guild.locale.is_some()),
        ),
        row(
            "Bot name",
            guild
                .bot_name
                .as_deref()
                .or(config.bot_name.as_deref())
                .unwrap_or("(not set)"),
            from(guild.bot_name.is_some()),
        ),
    ];
    rows.extend(command_rows(config, guild));
    rows.extend(ambient_rows(config, guild));
    rows.extend(feed_rows(config, guild));
    rows
}

fn command_rows(config: &Config, guild: &GuildConfig) -> Vec<Row> {
    let own = guild.commands.as_ref();
    let cmds = config.commands_for(guild.guild_id);
    vec![
        row(
            "/events",
            workflow(&cmds.events),
            from(own.is_some_and(|c| c.events.is_some())),
        ),
        row(
            "/locations",
            workflow(&cmds.locations),
            from(own.is_some_and(|c| c.locations.is_some())),
        ),
        row(
            "/set",
            on_off(cmds.set.enabled),
            from(own.is_some_and(|c| c.set.is_some())),
        ),
        row(
            "/part",
            on_off(cmds.part.enabled),
            from(own.is_some_and(|c| c.part.is_some())),
        ),
        row(
            "/diagnostics",
            on_off(cmds.diagnostics.enabled),
            from(own.is_some_and(|c| c.diagnostics.is_some())),
        ),
        row(
            "/build",
            if cmds.build.enabled {
                format!(
                    "on: models up to {size} studs wide, {size} bricks high",
                    size = cmds.build.max_model_size
                )
            } else {
                "off".to_string()
            },
            from(own.is_some_and(|c| c.build.is_some())),
        ),
    ]
}

fn ambient_rows(config: &Config, guild: &GuildConfig) -> Vec<Row> {
    let id = guild.guild_id;
    let ambient = guild.ambient.as_ref();
    let topics = config.ambient_topics_for(id);
    let training = config.ambient_training_data_for(id).map_or_else(
        || "off".to_string(),
        |t| {
            format!(
                "on: {}, {}",
                plural(t.retention_days as usize, "day", "days"),
                if t.store_raw_text {
                    "raw text"
                } else {
                    "no raw text"
                }
            )
        },
    );
    let channels = config.ambient_channel_ids_for(id).map_or_else(
        || "all".to_string(),
        |ids| {
            ids.iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        },
    );

    vec![
        row(
            "Ambient channels",
            channels,
            from(ambient.is_some_and(|a| a.channel_ids.is_some())),
        ),
        row(
            "Ambient /set offer",
            on_off(topics.set),
            from(ambient.is_some_and(|a| a.set.is_some())),
        ),
        row(
            "Ambient /part offer",
            on_off(topics.part),
            from(ambient.is_some_and(|a| a.part.is_some())),
        ),
        row(
            "Ambient debug log",
            on_off(config.ambient_debug_logging_for(id)),
            from(ambient.is_some_and(|a| a.debug_logging.is_some())),
        ),
        row(
            "Training data",
            training,
            from(ambient.is_some_and(|a| a.training_data.is_some())),
        ),
    ]
}

/// One row per channel the guild publishes feeds to, in the order the
/// channels first appear; a channel named by several sections gets one row.
fn feed_rows(config: &Config, guild: &GuildConfig) -> Vec<Row> {
    let mut channels: Vec<u64> = Vec::new();
    for section in &guild.feeds {
        if !channels.contains(&section.channel_id) {
            channels.push(section.channel_id);
        }
    }
    if channels.is_empty() {
        return vec![row("Feeds", "none", "-")];
    }
    channels
        .into_iter()
        .map(|channel| {
            let sections: Vec<&GuildFeedConfig> = guild
                .feeds
                .iter()
                .filter(|f| f.channel_id == channel)
                .collect();
            row(
                format!("Feeds → {channel}"),
                feed_channel(config, guild.guild_id, channel, &sections),
                from(sections.iter().any(|s| own_feed_settings(s))),
            )
        })
        .collect()
}

const fn follows_global(section: &GuildFeedConfig) -> bool {
    section.feed_urls.is_empty() && section.opml_urls.is_empty()
}

const fn own_feed_settings(section: &GuildFeedConfig) -> bool {
    !follows_global(section)
        || !section.blocked_domains.is_empty()
        || !section.blocked_feed_urls.is_empty()
        || section.schedule.is_some()
        || section.timezone.is_some()
}

fn sources(feed_urls: usize, opml_urls: usize) -> String {
    let mut parts = Vec::new();
    if feed_urls > 0 {
        parts.push(plural(feed_urls, "feed", "feeds"));
    }
    if opml_urls > 0 {
        parts.push(format!("{opml_urls} OPML"));
    }
    if parts.is_empty() {
        "no feeds".to_string()
    } else {
        parts.join(", ")
    }
}

fn feed_channel(
    config: &Config,
    guild_id: u64,
    channel: u64,
    sections: &[&GuildFeedConfig],
) -> String {
    let global = &config.feeds;
    let from_sections: Vec<String> = sections
        .iter()
        .map(|s| {
            let followed = if follows_global(s) {
                format!(
                    "global [feeds] ({})",
                    sources(global.feed_urls.len(), global.opml_urls.len())
                )
            } else {
                sources(s.feed_urls.len(), s.opml_urls.len())
            };
            // A section's own blocks cover only what that section follows.
            let domains = beyond(&s.blocked_domains, &global.blocked_domains);
            let feeds = beyond(&s.blocked_feed_urls, &global.blocked_feed_urls);
            let mut own = Vec::new();
            if domains > 0 {
                own.push(plural(domains, "domain", "domains"));
            }
            if feeds > 0 {
                own.push(plural(feeds, "feed", "feeds"));
            }
            if own.is_empty() {
                followed
            } else {
                format!("{followed} (also blocks {})", own.join(", "))
            }
        })
        .collect();
    // The global blocks cover every section.
    let blocks = if global.blocked_domains.is_empty() && global.blocked_feed_urls.is_empty() {
        "no blocks".to_string()
    } else {
        format!(
            "blocks {}, {}",
            plural(global.blocked_domains.len(), "domain", "domains"),
            plural(global.blocked_feed_urls.len(), "feed", "feeds")
        )
    };
    let schedule = config
        .feed_schedule_for_channel(guild_id, channel)
        .map_or_else(
            || "instant".to_string(),
            |cron| {
                format!(
                    "digest {cron} {}",
                    config.feed_timezone_for_channel(guild_id, channel)
                )
            },
        );
    format!("{}; {blocks}; {schedule}", from_sections.join(" + "))
}

/// How many of a section's own entries the global list does not already have.
fn beyond(own: &[String], global: &[String]) -> usize {
    own.iter()
        .filter(|o| !global.iter().any(|g| g.eq_ignore_ascii_case(o)))
        .count()
}

/// A URL as printed: without any user name or password it carries.
fn without_credentials(url: &str) -> String {
    reqwest::Url::parse(url).map_or_else(
        |_| url.to_string(),
        |mut parsed| {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        },
    )
}

/// Renders rows as a pipe table whose columns line up.
fn table(rows: &[Row]) -> String {
    let header = row("Setting", "Value", "From");
    let mut widths = [0; 3];
    for r in std::iter::once(&header).chain(rows) {
        for (width, cell) in widths.iter_mut().zip(r) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |r: &Row| {
        let cells: Vec<String> = r
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        format!("| {} |\n", cells.join(" | "))
    };
    let mut out = line(&header);
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    let _ = writeln!(out, "|-{}-|", rule.join("-|-"));
    for r in rows {
        out.push_str(&line(r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> Config {
        let config: Config = toml::from_str(toml).unwrap();
        config.validate().unwrap();
        config
    }

    /// The `Value` and `From` cells of the row whose setting is `name`, in
    /// the table that follows `heading`.
    fn row(report: &str, heading: &str, name: &str) -> (String, String) {
        let table = report
            .split(heading)
            .nth(1)
            .unwrap_or_else(|| panic!("no {heading:?} in:\n{report}"));
        let line = table
            .lines()
            .take_while(|l| !l.is_empty() || l.starts_with('|'))
            .find(|l| l.split('|').nth(1).is_some_and(|c| c.trim() == name))
            .unwrap_or_else(|| panic!("no {name:?} row under {heading:?} in:\n{report}"));
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        (cells[2].to_string(), cells[3].to_string())
    }

    const SAMPLE: &str = r#"
        locale = "fi-FI"
        bot_name = "Palikkabotti"

        [ambient]
        set = false
        part = false

        [commands.events]
        enabled = true
        enable_edit = true
        enable_fallback_mention = false

        [feeds]
        opml_urls = ["https://example.com/a.opml", "https://example.com/b.opml"]
        blocked_domains = ["kuvat.fi", "flickr.com"]
        blocked_feed_urls = ["https://example.com/bad.xml"]
        schedule = "0 9 * * *"

        [[guilds]]
        guild_id = 1

        [[guilds.feeds]]
        channel_id = 10

        [[guilds]]
        guild_id = 2
        locale = "en-US"

        [guilds.ambient]
        channel_ids = [20, 21]
        set = true
        debug_logging = true

        [guilds.ambient.training_data]
        enabled = true
        retention_days = 7

        [guilds.commands.diagnostics]
        enabled = true

        [guilds.commands.events]
        enabled = false

        [[guilds.feeds]]
        channel_id = 22
        feed_urls = ["https://example.com/own.xml"]
        blocked_domains = ["youtube.com"]
        schedule = "instant"
    "#;

    #[test]
    fn every_guild_gets_its_own_table() {
        let report = explain(&config(SAMPLE));
        assert!(report.contains("Guild 1\n"), "{report}");
        assert!(report.contains("Guild 2\n"), "{report}");
    }

    #[test]
    fn values_say_whether_the_guild_or_the_global_settings_gave_them() {
        let report = explain(&config(SAMPLE));
        assert_eq!(
            row(&report, "Guild 1\n", "Locale"),
            ("fi-FI".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 2\n", "Locale"),
            ("en-US".into(), "guild".into())
        );
        assert_eq!(
            row(&report, "Guild 1\n", "Bot name"),
            ("Palikkabotti".into(), "global".into())
        );
    }

    #[test]
    fn commands_show_their_options() {
        let report = explain(&config(SAMPLE));
        assert_eq!(
            row(&report, "Guild 1\n", "/events"),
            ("on: edit".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 2\n", "/events"),
            ("off".into(), "guild".into())
        );
        assert_eq!(
            row(&report, "Guild 1\n", "/diagnostics"),
            ("off".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 2\n", "/diagnostics"),
            ("on".into(), "guild".into())
        );
        assert_eq!(
            row(&report, "Guild 1\n", "/set"),
            ("on".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 1\n", "/locations"),
            ("off".into(), "global".into())
        );
    }

    #[test]
    fn ambient_settings_are_resolved_per_guild() {
        let report = explain(&config(SAMPLE));
        assert_eq!(
            row(&report, "Guild 1\n", "Ambient channels"),
            ("all".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 2\n", "Ambient channels"),
            ("20, 21".into(), "guild".into())
        );
        assert_eq!(
            row(&report, "Guild 1\n", "Ambient /set offer"),
            ("off".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 2\n", "Ambient /set offer"),
            ("on".into(), "guild".into())
        );
        // Guild 2 sets only `set`; `part` still comes from the global table.
        assert_eq!(
            row(&report, "Guild 2\n", "Ambient /part offer"),
            ("off".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 2\n", "Ambient debug log"),
            ("on".into(), "guild".into())
        );
        assert_eq!(
            row(&report, "Guild 1\n", "Training data"),
            ("off".into(), "global".into())
        );
        assert_eq!(
            row(&report, "Guild 2\n", "Training data"),
            ("on: 7 days, no raw text".into(), "guild".into())
        );
    }

    #[test]
    fn feed_channels_show_their_sources_blocks_and_schedule() {
        let report = explain(&config(SAMPLE));
        assert_eq!(
            row(&report, "Guild 1\n", "Feeds → 10"),
            (
                "global [feeds] (2 OPML); blocks 2 domains, 1 feed; digest 0 9 * * * Europe/Helsinki"
                    .into(),
                "global".into()
            )
        );
        assert_eq!(
            row(&report, "Guild 2\n", "Feeds → 22"),
            (
                "1 feed (also blocks 1 domain); blocks 2 domains, 1 feed; instant".into(),
                "guild".into()
            )
        );
        // A guild without feed sections publishes nothing.
        let quiet = explain(&config("[[guilds]]\nguild_id = 3"));
        assert_eq!(
            row(&quiet, "Guild 3\n", "Feeds"),
            ("none".into(), "-".into())
        );
    }

    #[test]
    fn the_global_settings_are_summarised_first() {
        let report = explain(&config(SAMPLE));
        let global = report.split("Guild 1\n").next().unwrap();
        assert!(global.starts_with("Global\n"), "{report}");
        assert_eq!(row(&report, "Global\n", "Feed polling").0, "every 3600 s");
        assert_eq!(row(&report, "Global\n", "Zulip").0, "off");
        assert_eq!(row(&report, "Global\n", "PocketBase").0, "off");
    }

    #[test]
    fn a_config_without_guilds_says_the_bot_answers_nowhere() {
        let report = explain(&config(""));
        assert!(report.contains("No guilds are configured"), "{report}");
    }

    #[test]
    fn columns_line_up() {
        let report = explain(&config(SAMPLE));
        let table: Vec<&str> = report
            .split("Guild 2\n")
            .nth(1)
            .unwrap()
            .lines()
            .take_while(|l| l.starts_with('|'))
            .collect();
        assert!(table.len() > 3);
        let width = table[0].chars().count();
        assert!(table.iter().all(|l| l.chars().count() == width), "{report}");
    }

    #[test]
    fn a_sections_own_blocks_are_shown_on_that_section_only() {
        let report = explain(&config(
            r#"
            [feeds]
            feed_urls = ["https://example.com/global.xml"]
            blocked_domains = ["kuvat.fi"]

            [[guilds]]
            guild_id = 1

            [[guilds.feeds]]
            channel_id = 10
            feed_urls = ["https://example.com/own.xml"]
            blocked_domains = ["youtube.com", "kuvat.fi"]

            [[guilds.feeds]]
            channel_id = 10
            "#,
        ));
        assert_eq!(
            row(&report, "Guild 1\n", "Feeds → 10").0,
            "1 feed (also blocks 1 domain) + global [feeds] (1 feed); blocks 1 domain, 0 feeds; instant"
        );
    }

    #[test]
    fn build_size_names_both_units() {
        let report = explain(&config(
            "[[guilds]]\nguild_id = 1\n[guilds.commands.build]\nenabled = true\nmax_model_size = 40",
        ));
        assert_eq!(
            row(&report, "Guild 1\n", "/build").0,
            "on: models up to 40 studs wide, 40 bricks high"
        );
    }

    #[test]
    fn urls_are_printed_without_credentials() {
        let report = explain(&config(
            r#"
            [pocketbase]
            url = "https://admin:hunter2@data.example.com/pb"
            collection = "events"
            "#,
        ));
        assert!(!report.contains("hunter2"), "{report}");
        assert!(!report.contains("admin"), "{report}");
        assert!(report.contains("https://data.example.com/pb"), "{report}");
    }
}
