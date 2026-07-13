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
```

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
- Polling logic was restructured to remove dependency on Discord `channel_id` directly in the feeds loop, paving the way for decoupled handling of fetched data.
