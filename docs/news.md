# RSS & ATOM Feed Polling

## Overview
The bot supports background polling of RSS and ATOM feeds, as well as OPML subscription lists. This allows the bot to continuously fetch new articles or updates and store them for future retrieval or processing.

## Supported Formats
- **RSS 2.0**: Parsed using the `rss` crate.
- **ATOM**: Parsed using the `atom_syndication` crate.
- **OPML 1.1/2.0**: Subscription lists parsed using the `opml` crate. The bot recursively extracts all `xmlUrl` attributes from the nested outline nodes to discover all subscribed feeds.

## Configuration
Feeds are configured in the `config.toml` file under the `[[feeds]]` array. The polling interval is controlled globally via `poll_interval`.

```toml
poll_interval = 600

[[feeds]]
feed_urls = [
    "https://example.com/rss",
    "https://example.com/atom"
]
opml_urls = [
    "https://example.com/subscriptions.opml"
]
# Never published to any channel (see "Blocked domains" below).
blocked_domains = ["flickr.com"]
```

## Publishing New Articles to a Guild

A guild opts in by naming a channel in a `[[guilds.feeds]]` section. Nothing is published anywhere without one.

```toml
[[guilds]]
guild_id = 123456789012345678

# These feeds' new articles go to this channel.
[[guilds.feeds]]
channel_id = 234567890123456789
feed_urls = ["https://example.com/rss"]
opml_urls = ["https://example.com/subscriptions.opml"]

# A section naming no URLs follows every global [[feeds]] entry.
[[guilds.feeds]]
channel_id = 345678901234567890
# Feeds served from these domains are not published to this channel, on top
# of the domains the global [[feeds]] block everywhere.
blocked_domains = ["kuvat.fi"]
```

- A guild may have any number of sections, and feeds named only here are polled too — they need not be repeated under the global `[[feeds]]`.
- **Announced source by source.** A polling pass collects everything new for a channel across all of its feeds and announces it one source (feed title) at a time, with no heading. A source with up to three new articles gets one message per article, `[title](<link>) — source · date`. A source with more than three gets a single message of its own — the source name and a colon, then one `- [title](<link>) · date` line per article (a feed without a title is called "Other"; feeds sharing a title count as one source). Individually announced articles are posted oldest first; sources follow the order of their feed URLs. So a pass sends at least one message per source that has something new. Link previews are suppressed, markdown in titles is escaped and mentions are inert. If a source's list would exceed Discord's message limit it ends with "…and N more" instead of spilling into a second message.
- **Titles.** Trailing hashtags (`Title #lego #shorts`) are dropped from announced titles, as the planet aggregator does; numeric ones such as `#123` and hashtags inside the title stay. A title that is nothing but hashtags is announced as the bare link.
- **Publication dates.** The date is the feed's own (`pubDate`, or failing that `dc:date`, in RSS; `published`, or failing that `updated`, in Atom; RFC 2822 or RFC 3339), sent as a Discord timestamp (`<t:…:d>`) so that each reader sees it in their own language and time zone. An article whose feed gives no date, one that cannot be read, or a placeholder (the Unix epoch or earlier, year 2100 or later) is announced without one.
- **Blocked domains.** `blocked_domains` keeps feeds out of a channel by where they are served from: the host of the *feed URL* (not of the article links) is compared case-insensitively, and a domain covers its subdomains — `kuvat.fi` blocks `palikkatakomo.kuvat.fi`, `flickr.com` does not block `notflickr.com`. The global `[[feeds]]` entries' `blocked_domains` are the defaults: together they apply to every section of every guild. A section's own `blocked_domains` are added to them for that section's channel only; a section cannot take a default away. Either way the list applies to whatever the section follows — its own `feed_urls`, the feeds of its `opml_urls`, or the global `[[feeds]]`. Entries must be bare domains (no scheme, port, path or wildcard; an internationalized name in its `xn--` form); anything else is rejected at startup and on reload. A blocked feed is still polled and stored, so its articles keep turning up in `/set`; it is only never announced.
- **Only articles that appear after startup.** What a feed has already shown is recorded in the `feed_sightings` table (`db::record_feed_sightings`), and `feed_polls.baseline_run` names the bot run that last took the feed's baseline. A feed's first successful poll in a run is that baseline and is silent; only articles first seen on a later poll of the same run are announced. Anything published before or while the bot was down is never mentioned — and since a `SIGHUP` reload re-executes the bot, a reload starts a fresh baseline as well.
- Sightings are kept only for feeds some guild publishes. A row is refreshed while its feed still lists the article and pruned 30 days after it last did, so the table is bounded by what the feeds carry.
- A published feed is polled once right at startup, regardless of its last poll time, so the baseline is taken immediately rather than up to one `poll_interval` later. That is one attempt: if it fails, the baseline is taken by the feed's next regular poll instead, and whatever was published in between is not announced.
- Every feed polled in a pass is stamped with the time the pass began, so feeds that are polled together fall due together again — which is what lets a channel's updates be batched together. A feed that does not answer within 30 seconds is skipped for that pass.
- A `channel_id` may appear in several sections of one guild, but not under two guilds. A section naming no URLs is rejected when there are no global `[[feeds]]` to follow.

**Known limitations**
- An article that reaches a channel through two different feeds in *different* passes is announced twice; duplicates are only merged within one pass.
- An article is identified by its link (its GUID only when it has none), so a feed that rewrites its links re-announces them.
- A message Discord rejects (missing permission, outage) is logged and not retried, and the rest of that pass's messages for the channel are dropped with it.
- There is no cap on messages per pass: N sources with news mean up to 3N messages, paced by Discord's rate limit.
- A link longer than 500 characters is left out; the article is named by its title alone.
- The fixed words ("Other", "…and N more") are localized with the guild's locale (`feeds.updates.*`).

## Polling Logic
- The `global_poll_loop` in `src/rss.rs` runs asynchronously as a single, global background task.
- Every `poll_interval` seconds, it iterates over all configured feeds and OPML URLs.
- It dynamically resolves OPML files into individual feed URLs.
- For each unique feed URL, it checks the `feed_polls` table in the SQLite database for its `last_polled_at` timestamp.
- If the feed's `poll_interval` has elapsed since its last poll (or if it has never been polled), the bot fetches it via the `HttpClient`, processes any new items, and updates the `last_polled_at` record.
- By tracking `last_polled_at` in the database, the bot remembers recent fetches across restarts, preventing immediate feed spam on startup.
- Fetched items are stored in the `feed_items` table and deduplicated.

## Recent Changes
- Support for ATOM feeds was recently introduced to resolve parsing issues with feeds from YouTube and Flickr.
- New articles can be published to per-guild channels (`[[guilds.feeds]]`), announced source by source: one message per article for up to three, one message listing them all beyond that.
- Announcements carry the article's publication date, and `blocked_domains` keeps feeds out of channels by domain — globally under `[[feeds]]`, with more per `[[guilds.feeds]]` section.
