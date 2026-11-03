CREATE TABLE audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp BIGINT NOT NULL,
    actor_id TEXT,
    target_id TEXT,
    session_id TEXT,
    source TEXT NOT NULL CHECK (source IN ('local','oidc','system')),
    kind TEXT NOT NULL CHECK (kind IN ('client','web','cli','unknown')),
    action TEXT NOT NULL CHECK (action IN ('create_user','bootstrap','update_user','change_password','reset_password','oidc_jit','oidc_mapping','policy_change','login','logout','revoke_session','revoke_all','prune_audit')),
    result TEXT NOT NULL CHECK (result IN ('success','failure')),
    reason TEXT CHECK (reason IN ('invalid_request','rate_limited','invalid_origin','oidc_state','oidc_provider','oidc_verification','oidc_expired','session_denied','invalid_credentials')),
    before_role TEXT CHECK (before_role IN ('none','admin','member')),
    after_role TEXT CHECK (after_role IN ('none','admin','member')),
    before_admitted BIGINT CHECK (before_admitted IN (0,1)),
    after_admitted BIGINT CHECK (after_admitted IN (0,1)),
    before_disabled BIGINT CHECK (before_disabled IN (0,1)),
    after_disabled BIGINT CHECK (after_disabled IN (0,1)),
    policy_version TEXT NOT NULL,
    revoked_count BIGINT NOT NULL CHECK (revoked_count >= 0)
);
CREATE INDEX audit_timestamp_idx ON audit_events(timestamp, id);
CREATE INDEX audit_actor_idx ON audit_events(actor_id, id);
CREATE INDEX audit_target_idx ON audit_events(target_id, id);
