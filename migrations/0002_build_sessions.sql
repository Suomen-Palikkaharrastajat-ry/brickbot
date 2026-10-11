-- Collaborative /build sessions.
--
-- `workflow_sessions` cannot be reused here: every one of its accessors
-- filters on `owner_user_id`, so only the session creator could read it. A
-- build session is collaborative by definition — anyone in the thread edits
-- the same model — so authorisation is by thread membership, which Discord
-- already enforces on the channel.

CREATE TABLE IF NOT EXISTS build_sessions (
    id TEXT PRIMARY KEY,
    guild_id TEXT NOT NULL,
    -- The thread the session listens in. A thread has its own channel id.
    channel_id TEXT NOT NULL,
    owner_user_id TEXT NOT NULL,
    model_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    -- How many revisions are currently in effect, i.e. where undo and redo
    -- have left the session; NULL means "every revision is live". It has to
    -- be session state rather than a flag on each revision: moving the cursor
    -- *forward* (redo, or reverting to a later revision) has to bring
    -- revisions back, and a per-row flag that the writer only ever sets can
    -- never do that.
    cursor INTEGER,
    -- The sentence that is waiting on a part the built-in catalogue could not
    -- resolve, kept so the `build_part_pick` dropdown has something to retry
    -- once the builder chooses. Cleared as soon as any message resolves
    -- without a pending lookup, so a stale sentence cannot be replayed days
    -- later.
    pending_sentence TEXT,
    pending_kind TEXT,
    -- Per-session override of how big the model may grow. Set once, from the
    -- optional `size` argument on `/build` itself, and never changed
    -- afterwards — resizing mid-session would leave earlier revisions
    -- validated against a limit that no longer applies. All three are set
    -- together or not at all: NULL means "no override", so the session falls
    -- back to the guild's `max_model_size` config.
    max_size_x INTEGER,
    max_size_y INTEGER,
    max_size_z INTEGER
);

-- The hot path: every message in a build thread looks up its session by
-- channel. The partial index also enforces one live session per thread.
CREATE UNIQUE INDEX IF NOT EXISTS idx_build_sessions_channel
    ON build_sessions(channel_id) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_build_sessions_expires ON build_sessions(expires_at);

-- The operation log. The model is a fold over these, so only the operations
-- are stored, never a materialised model: the derived shape can change
-- without a data migration.
CREATE TABLE IF NOT EXISTS build_revisions (
    session_id TEXT NOT NULL,
    -- Revision index, which is also the `rev` half of every PlacementId the
    -- operation creates.
    n INTEGER NOT NULL,
    author_user_id TEXT NOT NULL,
    command_json TEXT NOT NULL,
    -- Set when a single operation was undone out of order by
    -- `Undo alice's last edit.` — and only then; where undo and redo have
    -- left the session is `build_sessions.cursor`. The row stays so that
    -- revision indices, and therefore placement ids, remain stable.
    skipped INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (session_id, n)
);

CREATE INDEX IF NOT EXISTS idx_build_revisions_session ON build_revisions(session_id);
