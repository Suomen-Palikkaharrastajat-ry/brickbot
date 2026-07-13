CREATE TABLE IF NOT EXISTS feed_items (
    id TEXT PRIMARY KEY,
    source_title TEXT NOT NULL,
    item_title TEXT NOT NULL,
    item_description TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_feed_items_title ON feed_items(item_title);
CREATE INDEX IF NOT EXISTS idx_feed_items_desc ON feed_items(item_description);

CREATE TABLE IF NOT EXISTS ambient_cooldowns (
    channel_id INTEGER NOT NULL,
    topic TEXT NOT NULL,
    last_suggested_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ttl DATETIME,
    PRIMARY KEY (channel_id, topic)
);


CREATE TABLE IF NOT EXISTS ambient_user_preferences (
    user_id TEXT PRIMARY KEY,
    ignore_all BOOLEAN NOT NULL DEFAULT 0,
    preferred_services TEXT,
    training_opt_out BOOLEAN NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS ambient_item_cooldowns (
    channel_id INTEGER NOT NULL,
    topic TEXT NOT NULL,
    item_id TEXT NOT NULL,
    last_suggested_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ttl DATETIME,
    PRIMARY KEY (channel_id, topic, item_id)
);

CREATE TABLE IF NOT EXISTS ambient_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    original_message_content TEXT NOT NULL,
    detected_topic TEXT NOT NULL,
    status TEXT DEFAULT 'pending',
    corrected_topic TEXT,
    ttl DATETIME,
    confidence REAL,
    guild_pseudonym TEXT,
    channel_pseudonym TEXT,
    extracted_item_id TEXT
);

CREATE TABLE IF NOT EXISTS inputs (
    id TEXT PRIMARY KEY,             -- Internal UUID
    source_user_id TEXT,             -- Discord User ID (for notifications)
    discord_channel_id TEXT,         -- Discord Channel/Thread ID
    zulip_stream TEXT,               -- E.g., "map-moderation"
    zulip_topic TEXT,                -- Unique topic name
    zulip_message_id INT,            -- The bot's initial prompt message ID
    payload_json TEXT,               -- Raw Discord form data waiting for approval
    status TEXT DEFAULT 'pending',   -- pending, approved, rejected
    ttl DATETIME,                    -- Time-to-live for cleanup
    moderated_by TEXT,
    moderated_at DATETIME,
    moderation_action TEXT,
    moderation_message_id TEXT
);

CREATE TABLE IF NOT EXISTS hidden_events (
    guild_id TEXT NOT NULL,
    brick_id TEXT NOT NULL,
    PRIMARY KEY (guild_id, brick_id)
);

CREATE TABLE IF NOT EXISTS feed_polls (
    url TEXT PRIMARY KEY,
    last_polled_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS workflow_sessions (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    guild_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    consumed_at DATETIME
);
CREATE INDEX IF NOT EXISTS idx_workflow_sessions_expires ON workflow_sessions(expires_at);
CREATE INDEX IF NOT EXISTS idx_workflow_sessions_owner ON workflow_sessions(owner_user_id);

CREATE TABLE IF NOT EXISTS submission_notifications (
    id TEXT PRIMARY KEY,
    input_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    discord_message_id TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error_code INTEGER,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    sent_at DATETIME,
    FOREIGN KEY(input_id) REFERENCES inputs(id)
);
