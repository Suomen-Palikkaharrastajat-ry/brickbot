-- Pending feed announcements queued for channels with scheduled announcements.
-- YouTube Shorts are filtered out before insertion and never queued.
-- Duplicate arrivals across feeds are deduplicated by (channel_id, item_id).
CREATE TABLE IF NOT EXISTS feed_pending_announcements (
    channel_id INTEGER NOT NULL,
    guild_id INTEGER NOT NULL,
    item_id TEXT NOT NULL,
    feed_url TEXT NOT NULL,
    source_title TEXT NOT NULL,
    item_title TEXT NOT NULL,
    published_at DATETIME,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (channel_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_feed_pending_channel ON feed_pending_announcements(channel_id);

-- Tracks when a channel last completed an announcement run for its cron schedule.
CREATE TABLE IF NOT EXISTS feed_channel_announcements (
    channel_id INTEGER PRIMARY KEY,
    last_announced_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);
