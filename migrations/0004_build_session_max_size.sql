-- Per-session override of how big a `/build` model may grow.
--
-- Set once, from the optional `size` argument on `/build` itself, and never
-- changed afterwards — resizing mid-session would leave earlier revisions
-- validated against a limit that no longer applies. All three columns are
-- set together or not at all: NULL means "no override", so the session falls
-- back to the guild's `max_model_size` config, which is how every session
-- that predates this migration keeps behaving exactly as before.
ALTER TABLE build_sessions ADD COLUMN max_size_x INTEGER;
ALTER TABLE build_sessions ADD COLUMN max_size_y INTEGER;
ALTER TABLE build_sessions ADD COLUMN max_size_z INTEGER;
