CREATE TABLE devices (
    id TEXT PRIMARY KEY NOT NULL,
    rustdesk_id TEXT NOT NULL UNIQUE,
    uuid TEXT NOT NULL UNIQUE,
    owner_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    platform TEXT NOT NULL,
    name TEXT NOT NULL,
    device_type TEXT NOT NULL,
    alias TEXT NOT NULL DEFAULT '',
    last_login_at BIGINT NOT NULL
);
CREATE INDEX devices_owner_idx ON devices(owner_user_id);
ALTER TABLE sessions ADD COLUMN device_id TEXT REFERENCES devices(id) ON DELETE SET NULL;
CREATE TABLE book_credentials (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    peer_id TEXT NOT NULL,
    key_id TEXT NOT NULL,
    nonce TEXT NOT NULL,
    ciphertext TEXT NOT NULL,
    PRIMARY KEY (user_id, peer_id)
);
CREATE TABLE device_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp BIGINT NOT NULL,
    actor_id TEXT NOT NULL REFERENCES users(id),
    device_id TEXT NOT NULL REFERENCES devices(id),
    action TEXT NOT NULL CHECK (action IN ('assign', 'unassign', 'alias')),
    old_owner_id TEXT REFERENCES users(id),
    new_owner_id TEXT REFERENCES users(id),
    revoked_count BIGINT NOT NULL DEFAULT 0 CHECK (revoked_count >= 0)
);
