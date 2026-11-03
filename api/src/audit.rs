//! Closed audit vocabulary: credentials, names, claims and arbitrary metadata have no field.
use serde::{Deserialize, Serialize};

macro_rules! vocabulary {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $value)] $variant),+ }
        impl $name {
            pub(crate) fn as_str(self) -> &'static str { match self { $(Self::$variant => $value),+ } }
            pub(crate) fn parse(value: &str) -> Option<Self> { match value { $($value => Some(Self::$variant)),+, _ => None } }
        }
    };
}
vocabulary!(AuditSource { Local => "local", Oidc => "oidc", System => "system" });
vocabulary!(AuditKind { Client => "client", Web => "web", Cli => "cli", Unknown => "unknown" });
vocabulary!(AuditAction {
    CreateUser => "create_user", Bootstrap => "bootstrap", UpdateUser => "update_user",
    ChangePassword => "change_password", ResetPassword => "reset_password",
    OidcJit => "oidc_jit", OidcMapping => "oidc_mapping", PolicyChange => "policy_change",
    Login => "login", Logout => "logout", RevokeSession => "revoke_session",
    RevokeAll => "revoke_all", PruneAudit => "prune_audit"
});
vocabulary!(AuditResult { Success => "success", Failure => "failure" });
vocabulary!(AuditReason {
    InvalidRequest => "invalid_request", RateLimited => "rate_limited", InvalidOrigin => "invalid_origin",
    OidcState => "oidc_state", OidcProvider => "oidc_provider", OidcVerification => "oidc_verification",
    OidcExpired => "oidc_expired", SessionDenied => "session_denied", InvalidCredentials => "invalid_credentials"
});
vocabulary!(AuditRole { None => "none", Admin => "admin", Member => "member" });

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditFilter {
    pub before_id: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub actor_id: Option<String>,
    pub target_id: Option<String>,
    pub action: Option<AuditAction>,
    pub source: Option<AuditSource>,
    pub kind: Option<AuditKind>,
    pub result: Option<AuditResult>,
}
fn default_limit() -> u32 {
    50
}
impl Default for AuditFilter {
    fn default() -> Self {
        Self {
            before_id: None,
            limit: default_limit(),
            since: None,
            until: None,
            actor_id: None,
            target_id: None,
            action: None,
            source: None,
            kind: None,
            result: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub id: i64,
    pub timestamp: i64,
    pub actor_id: Option<String>,
    pub target_id: Option<String>,
    pub session_id: Option<String>,
    /// Actor authentication source, attempted login source, or system for CLI/background work.
    pub source: AuditSource,
    pub kind: AuditKind,
    pub action: AuditAction,
    pub result: AuditResult,
    pub reason: Option<AuditReason>,
    pub before_role: Option<AuditRole>,
    pub after_role: Option<AuditRole>,
    pub before_admitted: Option<bool>,
    pub after_admitted: Option<bool>,
    pub before_disabled: Option<bool>,
    pub after_disabled: Option<bool>,
    pub policy_version: String,
    pub revoked_count: i64,
}
#[derive(Debug, Serialize)]
pub struct AuditPage {
    pub events: Vec<AuditEvent>,
    pub next_before_id: Option<i64>,
}

pub(crate) struct AuditWrite<'a> {
    pub actor_id: Option<&'a str>,
    pub target_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub source: AuditSource,
    pub kind: AuditKind,
    pub action: AuditAction,
    pub result: AuditResult,
    pub reason: Option<AuditReason>,
    pub before: Option<(AuditRole, bool, bool)>,
    pub after: Option<(AuditRole, bool, bool)>,
    pub revoked_count: i64,
}
impl<'a> AuditWrite<'a> {
    pub fn new(source: AuditSource, kind: AuditKind, action: AuditAction) -> Self {
        Self {
            actor_id: None,
            target_id: None,
            session_id: None,
            source,
            kind,
            action,
            result: AuditResult::Success,
            reason: None,
            before: None,
            after: None,
            revoked_count: 0,
        }
    }
}
