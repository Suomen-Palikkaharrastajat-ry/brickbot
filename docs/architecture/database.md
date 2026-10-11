# Database Architecture

Brickbot relies on an embedded **SQLite** database via `sqlx` to maintain application state, ensure idempotency for background tasks, and persist user preferences.

## Schema & Migrations

The database schema is defined and managed using `sqlx::migrate!`. The migrations live in the `./migrations` directory. `0001_initial.sql` is the squashed original schema; `0002_build_sessions.sql` holds everything `/build` needs (also squashed — it had not reached production) and `0003_feed_sightings.sql` the feed publishing state. A migration that has been applied in production must never be edited: `sqlx` verifies checksums at startup.

**Key Tables:**
- `feed_items`: The articles fetched from RSS/ATOM feeds (title and description), searched by the set command's news lookup.
- `feed_polls`: Stores the timestamp of the last poll for each feed URL, enabling intelligent, stateful polling loops that survive bot restarts. `baseline_run` is the bot run (process start in Unix milliseconds) that last took the feed's silent baseline for publishing.
- `feed_sightings`: Per feed, the article ids it has shown (`feed_url`, `item_id`, `last_seen_at`), for feeds published to guild channels. A feed's rows not seen for 30 days are pruned on its own next successful poll; rows of feeds without a baseline in the current run are pruned by any poll.
- `inputs`: Serves as local draft storage for multi-step modal interactions (like event submissions). Submissions are marked as `draft` or `pending` (when sent to Zulip for moderation).
- `ambient_cooldowns` / `ambient_item_cooldowns`: Implements rate-limiting for the ambient conversational assistant to prevent channel noise.
- `ambient_user_preferences`: Tracks user opt-outs (`ignore_all`) for the ambient assistant.

- `ambient_logs`: Records ambient assistant detections (without violating privacy) for training and analytics.

## Data Lifecycle & Management

### Feed Polling & Item Tracking
1. The global RSS polling loop checks `feed_polls` to determine if a feed is due for an update based on the configured interval.
2. If due, the feed is fetched, and `feed_polls` is updated with `CURRENT_TIMESTAMP`.
3. Each item in the feed is upserted into `feed_items`, which backs the news lookup of the set command. Publishing new articles to guild channels does **not** consult this table: it uses `feed_sightings` and `feed_polls.baseline_run`, so that only articles first seen after the run's baseline poll are announced (see [`docs/news.md`](../news.md)).
4. Stale sources are periodically cleaned up using `cleanup_removed_sources` to prevent unbounded growth of `feed_items`.

### Event Synchronization
Event synchronization is stateless and relies on special markers embedded in the Scheduled Event description to map PocketBase events to Discord Scheduled Events, without touching manually created server events.

### Draft Storage
When a user begins interacting with the `/events` wizard, an entry is created in `inputs`. If the interaction is aborted or fails, the data remains safely in the database, enabling features like "Edit & Retry".
