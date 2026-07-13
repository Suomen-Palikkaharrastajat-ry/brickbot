# Database Architecture

Brickbot relies on an embedded **SQLite** database via `sqlx` to maintain application state, ensure idempotency for background tasks, and persist user preferences.

## Schema & Migrations

The database schema is defined and managed using `sqlx::migrate!`. The migrations live in the `./migrations` directory. There is currently a squashed `0001_initial.sql` file which contains the entire schema.

**Key Tables:**
- `feed_items`: Tracks previously seen RSS/ATOM feed GUIDs to prevent double-posting.
- `feed_polls`: Stores the timestamp of the last poll for each feed URL, enabling intelligent, stateful polling loops that survive bot restarts.
- `inputs`: Serves as local draft storage for multi-step modal interactions (like event submissions). Submissions are marked as `draft` or `pending` (when sent to Zulip for moderation).
- `ambient_cooldowns` / `ambient_item_cooldowns`: Implements rate-limiting for the ambient conversational assistant to prevent channel noise.
- `ambient_user_preferences`: Tracks user opt-outs (`ignore_all`) for the ambient assistant.

- `ambient_logs`: Records ambient assistant detections (without violating privacy) for training and analytics.

## Data Lifecycle & Management

### Feed Polling & Item Tracking
1. The global RSS polling loop checks `feed_polls` to determine if a feed is due for an update based on the configured interval.
2. If due, the feed is fetched, and `feed_polls` is updated with `CURRENT_TIMESTAMP`.
3. For each item in the feed, the bot checks `feed_items`. If the GUID exists, it is skipped. If new, it is processed, sent to Discord, and the GUID is inserted into `feed_items`.
4. Stale sources are periodically cleaned up using `cleanup_removed_sources` to prevent unbounded growth of `feed_items`.

### Event Synchronization
Event synchronization is stateless and relies on special markers embedded in the Scheduled Event description to map PocketBase events to Discord Scheduled Events, without touching manually created server events.

### Draft Storage
When a user begins interacting with the `/events` wizard, an entry is created in `inputs`. If the interaction is aborted or fails, the data remains safely in the database, enabling features like "Edit & Retry".
