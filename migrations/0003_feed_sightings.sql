-- Which articles each feed has already shown, so that publishing to guild
-- channels can tell a new article from one it has seen — without holding
-- every article id in memory for the life of the process.
--
-- Keyed per feed rather than reusing `feed_items` (keyed by article alone):
-- the same article can arrive through two feeds that publish to different
-- channels, and each must get to announce it.
--
-- `last_seen_at` is refreshed on every poll that still lists the article, and
-- rows not seen for 30 days are pruned, so the table stays bounded by what
-- the feeds currently carry instead of growing forever.
CREATE TABLE IF NOT EXISTS feed_sightings (
    feed_url TEXT NOT NULL,
    item_id TEXT NOT NULL,
    last_seen_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (feed_url, item_id)
);

CREATE INDEX IF NOT EXISTS idx_feed_sightings_last_seen ON feed_sightings(last_seen_at);

-- The bot run (process start, in Unix milliseconds) that last took this
-- feed's baseline. Only articles appearing after the latest restart are
-- announced, so a feed's first poll of each run records sightings silently
-- and stamps the run here; a later poll of the same run announces what is
-- new. NULL — every row that predates this migration — means "no baseline
-- yet", which is simply the state every feed is in after a restart.
ALTER TABLE feed_polls ADD COLUMN baseline_run INTEGER;
