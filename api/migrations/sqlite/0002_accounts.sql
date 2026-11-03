-- Legacy bearer tokens remain client credentials. Public IDs are backfilled by
-- Rust while holding the migration lock, before their unique index is created.
ALTER TABLE sessions ADD COLUMN id TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'client' CHECK (kind IN ('client', 'web'));
ALTER TABLE sessions ADD COLUMN created_at BIGINT NOT NULL DEFAULT 0;
INSERT INTO settings (key, value)
SELECT 'bootstrap_initialized', CASE WHEN EXISTS (
    SELECT 1 FROM users WHERE source = 'local' AND role = 'admin'
) THEN '1' ELSE '0' END;
