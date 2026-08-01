-- Schema for `data/cache.db`, which is a cache and nothing else.
--
-- Deliberately a separate database from `data/bot.db`. Everything here is
-- derived from the Rebrickable API and can be thrown away: deleting the file
-- is a clean rebuild, backups of the real state stay small, and the cache can
-- run looser pragmas than session data ever should.
--
-- Applied with an explicit `sqlx::migrate!("./migrations_cache")`. Pointing the
-- cache pool at `./migrations` would build the entire bot schema in here.

-- Parts we have ever seen, keyed by Rebrickable part number.
--
-- These never expire. A 2x4 brick's number and name do not change, and the
-- point of the cache is that autocomplete keeps working offline and across
-- restarts.
CREATE TABLE IF NOT EXISTS part_details (
    part_num     TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    -- Lowercased on write, so the index below is usable without calling a
    -- function on every row. Backs the second lookup path: prefix and
    -- substring matching over every part ever cached, which answers queries
    -- nobody has typed before.
    name_folded  TEXT NOT NULL,
    -- The whole API response, so a detail lookup needs no round trip either.
    -- Null when the part was only ever seen as a search result.
    payload_json TEXT,
    fetched_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_part_details_name_folded
    ON part_details(name_folded);

-- Searches we have run, keyed by the normalised query.
--
-- Unlike part details these do go stale, because Rebrickable adds parts. A
-- search that found nothing expires much sooner: a typo must not pin an empty
-- answer for a month, but it must not be re-fetched on every keystroke either.
CREATE TABLE IF NOT EXISTS part_searches (
    query          TEXT PRIMARY KEY,
    -- Part numbers, in the order the API returned them.
    part_nums_json TEXT NOT NULL,
    -- True when the search found nothing, so the short expiry can be explained
    -- rather than inferred from an empty list.
    is_miss        INTEGER NOT NULL DEFAULT 0,
    fetched_at     TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at     TIMESTAMP NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_part_searches_expires
    ON part_searches(expires_at);
