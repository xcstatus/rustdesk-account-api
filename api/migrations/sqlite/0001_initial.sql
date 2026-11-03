CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('', 'admin', 'member')),
    source TEXT NOT NULL CHECK (source IN ('local', 'oidc')),
    disabled BIGINT NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1)),
    admitted BIGINT NOT NULL DEFAULT 1 CHECK (admitted IN (0, 1)),
    password_hash TEXT,
    issuer TEXT,
    subject TEXT,
    UNIQUE (issuer, subject),
    CHECK ((source = 'local' AND password_hash IS NOT NULL AND issuer IS NULL AND subject IS NULL)
        OR (source = 'oidc' AND password_hash IS NULL AND issuer IS NOT NULL AND subject IS NOT NULL))
);
CREATE TABLE IF NOT EXISTS sessions (
    digest TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at BIGINT NOT NULL,
    revoked BIGINT NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1))
);
CREATE INDEX IF NOT EXISTS sessions_user_id_idx ON sessions(user_id);
CREATE TABLE IF NOT EXISTS books (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    snapshot TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
INSERT INTO settings (key, value) VALUES ('policy_version', '') ON CONFLICT (key) DO NOTHING;
