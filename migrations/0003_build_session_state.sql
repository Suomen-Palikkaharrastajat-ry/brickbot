-- Session-level state for /build that does not belong in the operation log.
--
-- Both columns exist because `build_revisions.skipped` was being asked to mean
-- two different things at once.
--
-- `cursor` is how many revisions are currently in effect, i.e. where undo and
-- redo have left the session. It has to be session state rather than a flag on
-- each revision: moving the cursor *forward* (redo, or reverting to a later
-- revision) has to bring revisions back, and a per-row "skipped" flag that the
-- writer only ever sets can never do that. `skipped` now means exactly one
-- thing: a single operation undone out of order by `Undo alice's last edit.`
--
-- NULL means "every revision is live", which is the correct reading for the
-- sessions that already exist when this migration runs.
ALTER TABLE build_sessions ADD COLUMN cursor INTEGER;

-- The sentence that is waiting on a part the built-in catalogue could not
-- resolve, kept so the `build_part_pick` dropdown has something to retry once
-- the builder chooses. Cleared as soon as any message resolves without a
-- pending lookup, so a stale sentence cannot be replayed days later.
ALTER TABLE build_sessions ADD COLUMN pending_sentence TEXT;
ALTER TABLE build_sessions ADD COLUMN pending_kind TEXT;
