#![allow(clippy::or_fun_call, unused_imports, clippy::cast_possible_wrap)]
use crate::http::{HttpClient, HttpProvider};
use anyhow::Result;
use rss::Channel;
use rust_i18n::t;
use serenity::all::{ChannelId, CreateAllowedMentions, CreateMessage, Framework, FutureExt, Http};
use sqlx::SqlitePool;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use tokio::time::{Duration, sleep, timeout};

use crate::config::Config;
use crate::db::insert_feed_item;

#[derive(Debug)]
pub enum Feed {
    Rss(Box<rss::Channel>),
    Atom(Box<atom_syndication::Feed>),
}

pub fn parse_feed(body: &[u8]) -> Result<Feed> {
    if let Ok(channel) = rss::Channel::read_from(body) {
        return Ok(Feed::Rss(Box::new(channel)));
    }

    if let Ok(feed) = atom_syndication::Feed::read_from(body) {
        return Ok(Feed::Atom(Box::new(feed)));
    }

    anyhow::bail!("the input could not be parsed as RSS or Atom");
}

pub async fn fetch_feed(client: &HttpClient, url: &str, limit: u64) -> Result<Feed> {
    let body = client.get_bounded_bytes(url, limit, false).await?;
    parse_feed(&body[..])
}

/// One article of a feed, reduced to what the bot stores and publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEntry {
    /// The article's link, or its GUID when it has none.
    pub id: String,
    pub source_title: String,
    pub title: String,
    pub description: String,
}

/// The articles of a parsed feed, skipping any that cannot be identified.
#[must_use]
pub fn feed_entries(feed: &Feed) -> Vec<FeedEntry> {
    let mut entries = Vec::new();
    match feed {
        Feed::Rss(channel) => {
            for item in channel.items() {
                entries.push(FeedEntry {
                    id: item
                        .link()
                        .or_else(|| item.guid().map(rss::Guid::value))
                        .unwrap_or_default()
                        .to_string(),
                    source_title: channel.title().to_string(),
                    title: item.title().unwrap_or_default().to_string(),
                    description: item.description().unwrap_or_default().to_string(),
                });
            }
        }
        Feed::Atom(feed) => {
            for entry in &feed.entries {
                entries.push(FeedEntry {
                    id: entry
                        .links
                        .first()
                        .map_or(entry.id.as_str(), |l| l.href.as_str())
                        .to_string(),
                    source_title: feed.title.to_string(),
                    title: entry.title.to_string(),
                    description: entry
                        .summary
                        .as_ref()
                        .map(atom_syndication::Text::as_str)
                        .or_else(|| entry.content.as_ref().and_then(|c| c.value.as_deref()))
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        }
    }
    entries.retain(|e| !e.id.trim().is_empty());
    entries
}

/// Fetches a feed, stores its articles and returns them.
pub async fn poll_once(
    db: &SqlitePool,
    feed_url: &str,
    client: &HttpClient,
    limit: u64,
) -> anyhow::Result<Vec<FeedEntry>> {
    let feed = fetch_feed(client, feed_url, limit).await?;
    let entries = feed_entries(&feed);

    for e in &entries {
        let _ = insert_feed_item(db, &e.id, &e.source_title, &e.title, &e.description).await;
    }

    Ok(entries)
}

/// The entries whose ids `record_feed_sightings` reported as unseen, each
/// once, in feed order.
#[must_use]
pub fn unseen_entries(entries: Vec<FeedEntry>, unseen: &[String]) -> Vec<FeedEntry> {
    let mut unseen: HashSet<&str> = unseen.iter().map(String::as_str).collect();
    entries
        .into_iter()
        .filter(|e| unseen.remove(e.id.as_str()))
        .collect()
}

/// A channel that a feed's new articles are published to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PublishTarget {
    pub channel_id: u64,
    pub guild_id: u64,
}

type OpmlCache = HashMap<String, Vec<String>>;

/// The feed URLs a list of feeds and OPML subscriptions stands for. An OPML
/// that has not been fetched yet contributes nothing.
fn resolve_feed_urls<'a>(
    feed_urls: &'a [String],
    opml_urls: &'a [String],
    opml_cache: &'a OpmlCache,
) -> impl Iterator<Item = &'a String> {
    feed_urls.iter().chain(
        opml_urls
            .iter()
            .filter_map(|url| opml_cache.get(url))
            .flatten(),
    )
}

/// Every OPML URL in the configuration, global and per guild.
#[must_use]
pub fn configured_opml_urls(config: &Config) -> BTreeSet<&str> {
    let global = config.feeds.iter().flat_map(|f| &f.opml_urls);
    let guild = config
        .guilds
        .iter()
        .flat_map(|g| &g.feeds)
        .flat_map(|f| &f.opml_urls);
    global.chain(guild).map(String::as_str).collect()
}

/// Every feed to poll: the global ones plus those only a guild subscribes to.
#[must_use]
pub fn feeds_to_poll(config: &Config, opml_cache: &OpmlCache) -> BTreeSet<String> {
    let global = config
        .feeds
        .iter()
        .flat_map(|f| resolve_feed_urls(&f.feed_urls, &f.opml_urls, opml_cache));
    let guild = config
        .guilds
        .iter()
        .flat_map(|g| &g.feeds)
        .flat_map(|f| resolve_feed_urls(&f.feed_urls, &f.opml_urls, opml_cache));
    global.chain(guild).cloned().collect()
}

/// Which channels each feed's new articles go to, per the guilds' `feeds`
/// sections. A section naming no URLs follows every global feed.
#[must_use]
pub fn publish_targets(
    config: &Config,
    opml_cache: &OpmlCache,
) -> HashMap<String, BTreeSet<PublishTarget>> {
    let mut targets: HashMap<String, BTreeSet<PublishTarget>> = HashMap::new();
    for guild in &config.guilds {
        for section in &guild.feeds {
            let target = PublishTarget {
                channel_id: section.channel_id,
                guild_id: guild.guild_id,
            };
            let urls: Vec<&String> = if section.feed_urls.is_empty() && section.opml_urls.is_empty()
            {
                config
                    .feeds
                    .iter()
                    .flat_map(|f| resolve_feed_urls(&f.feed_urls, &f.opml_urls, opml_cache))
                    .collect()
            } else {
                resolve_feed_urls(&section.feed_urls, &section.opml_urls, opml_cache).collect()
            };
            for url in urls {
                targets.entry(url.clone()).or_default().insert(target);
            }
        }
    }
    targets
}

/// Discord caps a message at 2000 characters. Lengths are measured in UTF-16
/// units, the most pessimistic way it could be counting, with some headroom.
const MAX_MESSAGE_CHARS: usize = 1900;
const MAX_TITLE_CHARS: usize = 150;

/// Gives up on a feed that does not answer, so that one hung server cannot
/// hold back what the same pass already found. `src/http.rs` has no timeout
/// of its own.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

fn message_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// A link longer than this is left out and the article named by title only,
/// so that no single article can crowd the others out of a message.
const MAX_LINK_CHARS: usize = 500;

/// Text on one line, with every run of whitespace a single space.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Flattens text onto one line and keeps it from closing a markdown link,
/// styling the lines after it, or turning its own line into a heading, quote
/// or list item.
fn inline_text(s: &str, max_chars: usize) -> String {
    let flat = one_line(s).replace('[', "(").replace(']', ")");
    let mut cut: String = flat.chars().take(max_chars).collect();
    if cut.len() < flat.len() {
        cut.pop();
        cut.push('…');
    }
    let mut escaped = String::with_capacity(cut.len());
    for (i, c) in cut.chars().enumerate() {
        let leads_a_block = i == 0 && matches!(c, '#' | '>' | '-');
        if leads_a_block || matches!(c, '\\' | '*' | '_' | '~' | '`' | '|') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Up to this many new articles of a source are announced one message each;
/// more than that are listed in a single message for the source.
const MAX_INDIVIDUAL_UPDATES: usize = 3;

/// An article as a link (or as plain text when it has no usable one).
fn article_text(entry: &FeedEntry) -> String {
    let linkable = (entry.id.starts_with("http://") || entry.id.starts_with("https://"))
        && !entry.id.contains(|c: char| c.is_whitespace() || c == '>')
        && message_len(&entry.id) <= MAX_LINK_CHARS;
    let title = inline_text(&entry.title, MAX_TITLE_CHARS);

    // Angle brackets keep Discord from unfurling a preview per article.
    match (linkable, title.is_empty()) {
        (true, false) => format!("[{title}](<{}>)", entry.id),
        (true, true) => format!("<{}>", entry.id),
        (false, false) => title,
        (false, true) => inline_text(&entry.id, MAX_TITLE_CHARS),
    }
}

/// One article announced on its own: `[title](<link>) — source`.
fn single_update_message(entry: &FeedEntry) -> String {
    let article = article_text(entry);
    let source = inline_text(&entry.source_title, MAX_TITLE_CHARS);
    if source.is_empty() {
        article
    } else {
        format!("{article} — {source}")
    }
}

/// Many articles of one source in a single message, under a line naming it.
///
/// Articles that would push it past Discord's length limit are summarised in
/// a closing "and N more" line rather than spilling into a second message.
fn source_updates_message(locale: &str, source: &str, entries: &[&FeedEntry]) -> String {
    // The name's own colon, if it ends in one, would only be doubled.
    let name = inline_text(source.trim_end_matches(':'), MAX_TITLE_CHARS);
    let mut message = if name.is_empty() {
        t!("feeds.updates.other", locale = locale).to_string()
    } else {
        name
    };
    message.push(':');

    let mut used = message_len(&message);
    for (i, entry) in entries.iter().enumerate() {
        let line = format!("\n- {}", article_text(entry));
        let remaining = entries.len() - i;
        let more = t!("feeds.updates.more", locale = locale, count = remaining).to_string();
        // Room is always kept for the closing line, unless this is the last
        // article and so needs none after it.
        let reserve = if remaining == 1 {
            0
        } else {
            message_len(&more) + 1
        };
        if used + message_len(&line) + reserve > MAX_MESSAGE_CHARS {
            message.push('\n');
            message.push_str(&more);
            break;
        }
        used += message_len(&line);
        message.push_str(&line);
    }
    message
}

/// The messages announcing what one poll found for a channel, source by
/// source in the order the sources first appear.
///
/// A source with a few new articles announces them one message each, oldest
/// first, as a chat is read. One with more than [`MAX_INDIVIDUAL_UPDATES`]
/// lists them in a single message of its own instead, so a busy source takes
/// up one message rather than many. Feeds sharing a title are one source.
#[must_use]
pub fn build_updates_messages(locale: &str, entries: &[FeedEntry]) -> Vec<String> {
    // Keyed by the name as it will be shown, so that two spellings rendering
    // alike are not announced as two sources.
    let mut sources: Vec<(String, Vec<&FeedEntry>)> = Vec::new();
    for entry in entries {
        let source = one_line(&entry.source_title);
        match sources.iter_mut().find(|(s, _)| *s == source) {
            Some((_, group)) => group.push(entry),
            None => sources.push((source, vec![entry])),
        }
    }

    let mut messages = Vec::new();
    for (source, group) in sources {
        if group.len() <= MAX_INDIVIDUAL_UPDATES {
            // Feeds list their newest article first.
            messages.extend(group.into_iter().rev().map(single_update_message));
        } else {
            messages.push(source_updates_message(locale, &source, &group));
        }
    }
    messages
}

/// Adds one feed's unseen articles to the batch of every channel it is
/// published to. A feed with nothing new adds nothing, not even an empty
/// batch, and an article already batched for a channel through another feed
/// is not repeated.
pub fn collect_updates(
    updates: &mut BTreeMap<PublishTarget, Vec<FeedEntry>>,
    targets: Option<&BTreeSet<PublishTarget>>,
    fresh: &[FeedEntry],
) {
    if fresh.is_empty() {
        return;
    }
    for target in targets.into_iter().flatten() {
        let batch = updates.entry(*target).or_default();
        for entry in fresh {
            if !batch.iter().any(|e| e.id == entry.id) {
                batch.push(entry.clone());
            }
        }
    }
}

fn extract_opml_urls(outlines: &[opml::Outline], urls: &mut Vec<String>) {
    for outline in outlines {
        if let Some(xml_url) = &outline.xml_url {
            urls.push(xml_url.clone());
        }
        extract_opml_urls(&outline.outlines, urls);
    }
}

/// Whether a URL polled at `last_polled_at` is due again at `now`.
fn is_due(
    last_polled_at: Option<chrono::NaiveDateTime>,
    now: chrono::NaiveDateTime,
    interval: u64,
) -> bool {
    last_polled_at.is_none_or(|last| (now - last).num_seconds() >= interval as i64)
}

pub async fn global_poll_loop(http: Arc<Http>, db: SqlitePool, config: Arc<Config>) {
    let client = HttpClient::new();
    let mut opml_cache: OpmlCache = HashMap::new();
    // Identifies this run in `feed_polls.baseline_run`.
    let run_id = chrono::Utc::now().timestamp_millis();
    let started_at = whole_seconds(chrono::Utc::now());
    let interval = config.poll_interval.unwrap_or(3600);
    let limit = config.resource_limits.max_http_body_bytes;

    loop {
        // Everything polled in a pass is stamped with the time the pass
        // began, not the time its own fetch finished. Feeds polled together
        // thus fall due together again, which is what keeps a channel's
        // updates batched in one pass instead of drifting into separate ones.
        let pass_at = whole_seconds(chrono::Utc::now());

        for opml_url in configured_opml_urls(&config) {
            let last = crate::db::get_last_polled_at(&db, opml_url).await;
            if opml_cache.contains_key(opml_url) && !is_due(last, pass_at, interval) {
                continue;
            }
            let fetched = timeout(
                FETCH_TIMEOUT,
                client.get_bounded_text(opml_url, limit, false),
            )
            .await;
            if let Ok(Ok(opml_content)) = fetched {
                if let Ok(document) = opml::OPML::from_str(&opml_content) {
                    let mut extracted = Vec::new();
                    extract_opml_urls(&document.body.outlines, &mut extracted);
                    opml_cache.insert(opml_url.to_string(), extracted);

                    let _ = crate::db::mark_polled(&db, opml_url, pass_at).await;
                } else {
                    tracing::error!("failed to parse OPML from {}", opml_url);
                }
            } else {
                tracing::error!("failed to fetch OPML from {}", opml_url);
            }
        }

        let targets = publish_targets(&config, &opml_cache);
        let mut updates: BTreeMap<PublishTarget, Vec<FeedEntry>> = BTreeMap::new();

        for feed_url in feeds_to_poll(&config, &opml_cache) {
            let feed_targets = targets.get(&feed_url);
            // A published feed is polled once right after startup, whatever
            // its interval says: updates count from that baseline, so it must
            // not wait out an interval left over from the previous process.
            let needs_baseline = feed_targets.is_some()
                && crate::db::needs_baseline_poll(&db, &feed_url, run_id, started_at).await;
            let last = crate::db::get_last_polled_at(&db, &feed_url).await;
            if !needs_baseline && !is_due(last, pass_at, interval) {
                continue;
            }

            let polled = timeout(FETCH_TIMEOUT, poll_once(&db, &feed_url, &client, limit))
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out")));
            match polled {
                // Sightings are only kept for feeds somebody publishes.
                Ok(entries) if feed_targets.is_some() => {
                    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
                    match crate::db::record_feed_sightings(&db, &feed_url, run_id, &ids).await {
                        Ok(unseen) => {
                            let fresh = unseen_entries(entries, &unseen);
                            collect_updates(&mut updates, feed_targets, &fresh);
                        }
                        Err(err) => {
                            tracing::error!("failed to record sightings of {}: {}", feed_url, err);
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => tracing::error!("poll error for {}: {}", feed_url, err),
            }

            let _ = crate::db::mark_polled(&db, &feed_url, pass_at).await;
        }

        for (target, entries) in updates {
            let locale = config.locale_or_default(target.guild_id);
            // Suppressed outright as well as link by link: a bare URL inside
            // a title would otherwise still unfurl.
            for content in build_updates_messages(locale, &entries) {
                let message = CreateMessage::new()
                    .content(content)
                    .flags(serenity::all::MessageFlags::SUPPRESS_EMBEDS)
                    .allowed_mentions(CreateAllowedMentions::new());
                if let Err(err) = ChannelId::new(target.channel_id)
                    .send_message(&http, message)
                    .await
                {
                    tracing::error!(
                        "failed to publish feed updates to channel {}: {}",
                        target.channel_id,
                        err
                    );
                    // A channel that refuses one message refuses the rest.
                    break;
                }
            }
        }

        sleep(Duration::from_mins(1)).await;
    }
}

/// `SQLite`'s `CURRENT_TIMESTAMP` has whole seconds only; times compared with
/// it are cut to match.
fn whole_seconds(at: chrono::DateTime<chrono::Utc>) -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp(at.timestamp(), 0)
        .unwrap_or(at)
        .naive_utc()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_opml_urls() {
        let opml_str = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
        <opml version="2.0">
            <head>
                <title>mySubscriptions.opml</title>
            </head>
            <body>
                <outline text="News">
                    <outline text="CNN" type="rss" xmlUrl="http://rss.cnn.com/rss/cnn_topstories.rss" />
                    <outline text="NYT" type="rss" xmlUrl="https://rss.nytimes.com/services/xml/rss/nyt/HomePage.xml" />
                </outline>
            </body>
        </opml>"#;

        let document = opml::OPML::from_str(opml_str).unwrap();
        let mut urls = Vec::new();
        extract_opml_urls(&document.body.outlines, &mut urls);

        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "http://rss.cnn.com/rss/cnn_topstories.rss");
        assert_eq!(
            urls[1],
            "https://rss.nytimes.com/services/xml/rss/nyt/HomePage.xml"
        );
    }

    #[test]
    fn test_parse_rss_feed() {
        let rss_data = r#"<?xml version="1.0" encoding="UTF-8" ?>
        <rss version="2.0">
        <channel>
          <title>W3Schools Home Page</title>
          <link>https://www.w3schools.com</link>
          <description>Free web building tutorials</description>
          <item>
            <title>RSS Tutorial</title>
            <link>https://www.w3schools.com/xml/xml_rss.asp</link>
            <description>New RSS tutorial on W3Schools</description>
          </item>
        </channel>
        </rss>"#;

        let feed = parse_feed(rss_data.as_bytes()).expect("Failed to parse valid RSS");
        if let Feed::Rss(channel) = feed {
            assert_eq!(channel.title, "W3Schools Home Page");
            assert_eq!(channel.items.len(), 1);
            assert_eq!(channel.items[0].title.as_deref(), Some("RSS Tutorial"));
        } else {
            panic!("Expected Feed::Rss");
        }
    }

    #[test]
    fn test_parse_atom_feed() {
        let atom_data = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <title>Example Feed</title>
          <link href="http://example.org/"/>
          <updated>2003-12-13T18:30:02Z</updated>
          <author>
            <name>John Doe</name>
          </author>
          <id>urn:uuid:60a76c80-d399-11d9-b93C-0003939e0af6</id>
          <entry>
            <title>Atom-Powered Robots Run Amok</title>
            <link href="http://example.org/2003/12/13/atom03"/>
            <id>urn:uuid:1225c695-cfb8-4ebb-aaaa-80da344efa6a</id>
            <updated>2003-12-13T18:30:02Z</updated>
            <summary>Some text.</summary>
          </entry>
        </feed>"#;

        let feed = parse_feed(atom_data.as_bytes()).expect("Failed to parse valid ATOM");
        if let Feed::Atom(feed) = feed {
            assert_eq!(feed.title.as_str(), "Example Feed");
            assert_eq!(feed.entries.len(), 1);
            assert_eq!(
                feed.entries[0].title.as_str(),
                "Atom-Powered Robots Run Amok"
            );
        } else {
            panic!("Expected Feed::Atom");
        }
    }

    #[test]
    fn test_parse_feed_invalid() {
        let invalid_data = "This is not an XML document";
        let err = parse_feed(invalid_data.as_bytes()).expect_err("Expected parse error");
        assert_eq!(
            err.to_string(),
            "the input could not be parsed as RSS or Atom"
        );
    }

    fn entry(id: &str, title: &str) -> FeedEntry {
        FeedEntry {
            id: id.to_string(),
            source_title: "Source".to_string(),
            title: title.to_string(),
            description: String::new(),
        }
    }

    #[test]
    fn feed_entries_reads_rss_and_skips_unidentifiable_items() {
        let rss_data = r#"<?xml version="1.0" encoding="UTF-8" ?>
        <rss version="2.0"><channel>
          <title>Bricks</title><link>https://example.com</link><description>d</description>
          <item><title>Linked</title><link>https://example.com/a</link><description>Body</description></item>
          <item><title>Guid only</title><guid>urn:b</guid></item>
          <item><title>Nothing to go by</title></item>
        </channel></rss>"#;

        let entries = feed_entries(&parse_feed(rss_data.as_bytes()).unwrap());

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "https://example.com/a");
        assert_eq!(entries[0].source_title, "Bricks");
        assert_eq!(entries[0].title, "Linked");
        assert_eq!(entries[0].description, "Body");
        assert_eq!(entries[1].id, "urn:b");
    }

    #[test]
    fn feed_entries_reads_atom() {
        let atom_data = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <title>Example Feed</title><id>urn:feed</id><updated>2003-12-13T18:30:02Z</updated>
          <entry>
            <title>Robots</title><link href="http://example.org/robots"/>
            <id>urn:1</id><updated>2003-12-13T18:30:02Z</updated><summary>Some text.</summary>
          </entry>
          <entry>
            <title>No link</title><id>urn:2</id><updated>2003-12-13T18:30:02Z</updated>
          </entry>
        </feed>"#;

        let entries = feed_entries(&parse_feed(atom_data.as_bytes()).unwrap());

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "http://example.org/robots");
        assert_eq!(entries[0].source_title, "Example Feed");
        assert_eq!(entries[0].description, "Some text.");
        assert_eq!(entries[1].id, "urn:2");
    }

    #[test]
    fn unseen_entries_keeps_feed_order_and_drops_repeats() {
        let entries = vec![
            entry("c", "C"),
            entry("a", "A"),
            entry("b", "B"),
            entry("c", "C"),
        ];

        let fresh = unseen_entries(entries, &["b".to_string(), "c".to_string()]);

        assert_eq!(fresh, vec![entry("c", "C"), entry("b", "B")]);
    }

    fn feeds_config() -> Config {
        toml::from_str(
            r#"
            [[feeds]]
            feed_urls = ["https://global/rss"]
            opml_urls = ["https://global/opml"]

            [[guilds]]
            guild_id = 1

            [[guilds.feeds]]
            channel_id = 10
            feed_urls = ["https://own/rss", "https://global/rss"]
            opml_urls = ["https://own/opml"]

            [[guilds]]
            guild_id = 2

            [[guilds.feeds]]
            channel_id = 20

            [[guilds]]
            guild_id = 3
            "#,
        )
        .unwrap()
    }

    fn opml_cache() -> OpmlCache {
        HashMap::from([
            (
                "https://global/opml".to_string(),
                vec!["https://global/from-opml".to_string()],
            ),
            (
                "https://own/opml".to_string(),
                vec!["https://own/from-opml".to_string()],
            ),
        ])
    }

    #[test]
    fn guild_feeds_are_polled_alongside_the_global_ones() {
        let config = feeds_config();

        assert_eq!(
            configured_opml_urls(&config),
            BTreeSet::from(["https://global/opml", "https://own/opml"])
        );
        assert_eq!(
            feeds_to_poll(&config, &opml_cache())
                .into_iter()
                .collect::<Vec<_>>(),
            [
                "https://global/from-opml",
                "https://global/rss",
                "https://own/from-opml",
                "https://own/rss",
            ]
        );
        // An OPML not fetched yet contributes nothing.
        assert_eq!(feeds_to_poll(&config, &HashMap::new()).len(), 2);
    }

    #[test]
    fn publish_targets_follow_each_guilds_feeds_sections() {
        let targets = publish_targets(&feeds_config(), &opml_cache());
        let one = PublishTarget {
            channel_id: 10,
            guild_id: 1,
        };
        let two = PublishTarget {
            channel_id: 20,
            guild_id: 2,
        };

        assert_eq!(targets.len(), 4);
        assert_eq!(targets["https://own/rss"], BTreeSet::from([one]));
        assert_eq!(targets["https://own/from-opml"], BTreeSet::from([one]));
        // A section without URLs follows every global feed.
        assert_eq!(targets["https://global/rss"], BTreeSet::from([one, two]));
        assert_eq!(targets["https://global/from-opml"], BTreeSet::from([two]));
    }

    #[test]
    fn without_guild_sections_nothing_is_published() {
        let config: Config = toml::from_str(
            r#"
            [[feeds]]
            feed_urls = ["https://global/rss"]

            [[guilds]]
            guild_id = 1
            "#,
        )
        .unwrap();

        assert!(publish_targets(&config, &HashMap::new()).is_empty());
    }

    fn sourced(id: &str, title: &str, source: &str) -> FeedEntry {
        FeedEntry {
            source_title: source.to_string(),
            ..entry(id, title)
        }
    }

    /// The one message a source with too much to announce individually gets.
    fn grouped(locale: &str, entries: &[FeedEntry]) -> String {
        let mut messages = build_updates_messages(locale, entries);
        assert_eq!(messages.len(), 1);
        messages.remove(0)
    }

    #[test]
    fn nothing_new_means_no_message() {
        assert!(build_updates_messages("en-US", &[]).is_empty());
    }

    #[test]
    fn one_article_is_announced_on_its_own_without_a_heading() {
        let messages = build_updates_messages("en-US", &[entry("https://example.com/a", "First")]);

        assert_eq!(messages, ["[First](<https://example.com/a>) — Source"]);
    }

    #[test]
    fn up_to_three_articles_of_a_source_get_a_message_each() {
        let messages = build_updates_messages(
            "en-US",
            &[
                entry("https://example.com/a", "First"),
                entry("https://example.com/b", "Second"),
                entry("urn:c", "Third"),
            ],
        );

        // Feeds list the newest first; the channel reads oldest first.
        assert_eq!(
            messages,
            [
                "Third — Source",
                "[Second](<https://example.com/b>) — Source",
                "[First](<https://example.com/a>) — Source",
            ]
        );
    }

    #[test]
    fn the_fourth_article_of_a_source_turns_messages_into_a_list() {
        let articles = |n: usize| -> Vec<FeedEntry> {
            (0..n)
                .map(|i| entry(&format!("https://example.com/{i}"), "T"))
                .collect()
        };

        assert_eq!(build_updates_messages("en-US", &articles(3)).len(), 3);
        assert_eq!(build_updates_messages("en-US", &articles(4)).len(), 1);
    }

    #[test]
    fn more_than_three_articles_of_a_source_share_one_message() {
        let entries: Vec<FeedEntry> = (1..=4)
            .map(|i| {
                sourced(
                    &format!("https://a.example/{i}"),
                    &format!("A {i}"),
                    "Brothers Brick",
                )
            })
            .collect();

        assert_eq!(
            grouped("en-US", &entries),
            "Brothers Brick:\n\
             - [A 1](<https://a.example/1>)\n\
             - [A 2](<https://a.example/2>)\n\
             - [A 3](<https://a.example/3>)\n\
             - [A 4](<https://a.example/4>)"
        );
    }

    #[test]
    fn each_source_is_announced_separately() {
        // Interleaved as two feeds polled in one pass never are, to show that
        // the sources are told apart by name, not by position.
        let mut entries = vec![sourced("https://b.example/1", "B 1", "Brickset")];
        entries.extend((1..=4).map(|i| {
            sourced(
                &format!("https://a.example/{i}"),
                &format!("A {i}"),
                "Brothers Brick",
            )
        }));
        entries.push(sourced("urn:b2", "B 2", " Brickset "));

        let messages = build_updates_messages("en-US", &entries);

        // Brickset has two, so each stands alone; Brothers Brick has four,
        // so they are listed together — in a message of their own.
        assert_eq!(
            messages,
            [
                "B 2 — Brickset",
                "[B 1](<https://b.example/1>) — Brickset",
                "Brothers Brick:\n\
                 - [A 1](<https://a.example/1>)\n\
                 - [A 2](<https://a.example/2>)\n\
                 - [A 3](<https://a.example/3>)\n\
                 - [A 4](<https://a.example/4>)",
            ]
        );
    }

    #[test]
    fn two_busy_sources_get_a_message_each() {
        let entries: Vec<FeedEntry> = (0..8)
            .map(|i| {
                sourced(
                    &format!("https://example.com/{i}"),
                    "T",
                    if i < 4 { "One" } else { "Two" },
                )
            })
            .collect();

        let messages = build_updates_messages("en-US", &entries);

        assert_eq!(messages.len(), 2);
        assert!(messages[0].starts_with("One:\n- "));
        assert!(messages[1].starts_with("Two:\n- "));
        assert_eq!(messages[1].lines().count(), 5);
    }

    #[test]
    fn articles_of_a_nameless_source_are_listed_under_a_localized_heading() {
        let entries: Vec<FeedEntry> = (0..4)
            .map(|i| sourced(&format!("https://example.com/{i}"), "T", ""))
            .collect();

        assert!(grouped("en-US", &entries).starts_with("Other:\n- "));
        assert!(grouped("fi-FI", &entries).starts_with("Muut:\n- "));
    }

    #[test]
    fn titles_cannot_break_the_link() {
        let odd = sourced("https://example.com/a", "  A [LEGO]\n review  ", "One");
        let untitled = sourced("https://example.com/b", "", "Two");
        let bare = sourced("urn:c", "", "");

        let messages = build_updates_messages("en-US", &[odd, untitled, bare]);

        assert_eq!(
            messages,
            [
                "[A (LEGO) review](<https://example.com/a>) — One",
                "<https://example.com/b> — Two",
                "urn:c",
            ]
        );
    }

    #[test]
    fn titles_cannot_break_the_list() {
        let mut entries: Vec<FeedEntry> = (0..3)
            .map(|i| entry(&format!("https://example.com/{i}"), "T"))
            .collect();
        entries.push(entry("https://example.com/odd", " A [LEGO]\n review "));

        let message = grouped("en-US", &entries);

        assert_eq!(message.lines().count(), 5);
        assert!(message.ends_with("\n- [A (LEGO) review](<https://example.com/odd>)"));
    }

    #[test]
    fn text_that_starts_a_message_cannot_become_a_heading_or_quote() {
        let alone = sourced("urn:a", "# Sale", "");
        let listed: Vec<FeedEntry> = (0..4)
            .map(|i| sourced(&format!("https://example.com/{i}"), "- T", "> Blog_news:"))
            .collect();

        assert_eq!(build_updates_messages("en-US", &[alone]), [r"\# Sale"]);
        let message = grouped("en-US", &listed);
        // The name is escaped, and its own colon is not doubled.
        assert!(message.starts_with("\\> Blog\\_news:\n- [\\- T](<https://example.com/0>)"));
    }

    #[test]
    fn spellings_that_render_alike_are_one_source() {
        let mut entries: Vec<FeedEntry> = (0..2)
            .map(|i| sourced(&format!("https://a.example/{i}"), "T", "Brick  Blog"))
            .collect();
        entries.extend(
            (0..2).map(|i| sourced(&format!("https://b.example/{i}"), "T", " Brick Blog\n")),
        );

        let message = grouped("en-US", &entries);

        assert!(message.starts_with("Brick Blog:\n"));
        assert_eq!(message.lines().count(), 5);
    }

    #[test]
    fn an_overlong_link_does_not_hide_the_articles_after_it() {
        let long = format!("https://example.com/{}", "a".repeat(3000));
        let mut entries = vec![entry(&long, "Long one")];
        entries.extend((0..4).map(|i| entry(&format!("https://example.com/{i}"), "T")));

        let message = grouped("en-US", &entries);

        assert!(message.starts_with("Source:\n- Long one\n- [T](<https://example.com/0>)"));
        assert_eq!(message.lines().count(), 6);
        assert!(!message.contains("more"));
    }

    #[test]
    fn an_untitled_article_with_an_overlong_link_is_still_named() {
        let long = format!("https://example.com/{}", "a".repeat(3000));

        let messages = build_updates_messages("en-US", &[sourced(&long, "", "")]);

        assert_eq!(messages.len(), 1);
        assert!(messages[0].starts_with("https://example.com/aaa"));
        assert!(messages[0].ends_with('…'));
        assert!(message_len(&messages[0]) <= MAX_TITLE_CHARS);
    }

    #[test]
    fn an_article_identified_by_whitespace_alone_is_skipped() {
        let rss_data = r#"<?xml version="1.0" encoding="UTF-8" ?>
        <rss version="2.0"><channel>
          <title>Bricks</title><link>https://example.com</link><description>d</description>
          <item><title>Blank guid</title><guid>   </guid></item>
        </channel></rss>"#;

        assert!(feed_entries(&parse_feed(rss_data.as_bytes()).unwrap()).is_empty());
    }

    #[test]
    fn long_titles_are_shortened() {
        let messages =
            build_updates_messages("en-US", &[entry("https://example.com/a", &"x".repeat(500))]);

        assert!(messages[0].contains(&format!("[{}…]", "x".repeat(MAX_TITLE_CHARS - 1))));
    }

    #[test]
    fn a_link_too_long_to_send_is_left_out() {
        let url = format!("https://example.com/{}", "a".repeat(3000));

        let messages = build_updates_messages("en-US", &[entry(&url, "Title")]);

        assert_eq!(messages, ["Title — Source"]);
    }

    #[test]
    fn an_overlong_batch_stays_one_message_and_counts_the_rest() {
        let entries: Vec<FeedEntry> = (0..200)
            .map(|i| entry(&format!("https://example.com/{i}"), &"t".repeat(100)))
            .collect();

        let message = grouped("en-US", &entries);

        assert!(message_len(&message) <= MAX_MESSAGE_CHARS);
        let listed = message.lines().filter(|l| l.starts_with("- ")).count();
        assert!(listed > 0 && listed < 200);
        assert!(message.starts_with("Source:\n- "));
        assert!(message.ends_with(&format!("\n…and {} more", 200 - listed)));
    }

    #[test]
    fn a_batch_that_fits_has_no_closing_line() {
        let entries: Vec<FeedEntry> = (0..5)
            .map(|i| entry(&format!("https://example.com/{i}"), "Title"))
            .collect();

        let message = grouped("en-US", &entries);

        assert_eq!(message.lines().count(), 6);
        assert!(!message.contains("more"));
    }

    #[test]
    fn the_closing_line_is_localized() {
        let entries: Vec<FeedEntry> = (0..200)
            .map(|i| entry(&format!("https://example.com/{i}"), &"t".repeat(100)))
            .collect();

        let message = grouped("fi-FI", &entries);

        assert!(message.lines().last().unwrap().starts_with("…ja "));
        assert!(message.ends_with(" muuta"));
    }

    #[test]
    fn the_length_limit_counts_utf16_units() {
        // Each of these is one `char` but two UTF-16 units.
        let entries: Vec<FeedEntry> = (0..40)
            .map(|i| entry(&format!("https://example.com/{i}"), &"🧱".repeat(100)))
            .collect();

        let message = grouped("en-US", &entries);

        assert!(message.encode_utf16().count() <= MAX_MESSAGE_CHARS);
        assert!(message.contains("more"));
    }

    #[test]
    fn markdown_in_a_title_is_escaped() {
        let messages = build_updates_messages(
            "en-US",
            &[entry(
                "https://example.com/a",
                r"*New* set_42 `x` ||y|| ~~z~~ \",
            )],
        );

        assert_eq!(
            messages[0],
            r"[\*New\* set\_42 \`x\` \|\|y\|\| \~\~z\~\~ \\](<https://example.com/a>) — Source"
        );
    }

    #[test]
    fn a_link_with_parentheses_is_still_a_link() {
        let messages =
            build_updates_messages("en-US", &[entry("https://example.com/Lego_(toy)", "Lego")]);

        assert!(messages[0].contains("[Lego](<https://example.com/Lego_(toy)>)"));
    }

    const ONE: PublishTarget = PublishTarget {
        channel_id: 10,
        guild_id: 1,
    };
    const TWO: PublishTarget = PublishTarget {
        channel_id: 20,
        guild_id: 2,
    };

    #[test]
    fn a_feed_with_nothing_new_announces_nothing() {
        let mut updates = BTreeMap::new();

        collect_updates(&mut updates, Some(&BTreeSet::from([ONE, TWO])), &[]);

        assert!(updates.is_empty());
    }

    #[test]
    fn an_unpublished_feed_announces_nothing() {
        let mut updates = BTreeMap::new();

        collect_updates(&mut updates, None, &[entry("a", "A")]);

        assert!(updates.is_empty());
    }

    #[test]
    fn a_pass_batches_every_feed_of_a_channel_together() {
        let mut updates = BTreeMap::new();

        collect_updates(
            &mut updates,
            Some(&BTreeSet::from([ONE, TWO])),
            &[entry("a", "A")],
        );
        // A second feed, published to the first channel only, carrying one
        // article the channel already has from the first feed.
        collect_updates(
            &mut updates,
            Some(&BTreeSet::from([ONE])),
            &[entry("a", "A"), entry("b", "B")],
        );

        assert_eq!(updates[&ONE], vec![entry("a", "A"), entry("b", "B")]);
        assert_eq!(updates[&TWO], vec![entry("a", "A")]);
    }

    #[test]
    fn a_feed_is_due_once_its_interval_has_passed() {
        let now = chrono::Utc::now().naive_utc();
        let ago = |secs| Some(now - chrono::Duration::seconds(secs));

        assert!(is_due(None, now, 3600));
        assert!(is_due(ago(3600), now, 3600));
        assert!(!is_due(ago(3599), now, 3600));
    }
}
