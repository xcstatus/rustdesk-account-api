//! Persistent account state. Bearers are digested, login passwords hashed, and book credentials sealed.
use crate::credentials::{CredentialCipher, SealedCredential};
use anyhow::{Context, Result, ensure};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[path = "audit.rs"]
pub mod audit;
pub use audit::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Any, AnyPool, Row, Transaction, any::AnyPoolOptions};
use uuid::Uuid;

// Keep dialect migration histories independent even while the initial schema is shared.
const SQLITE_INITIAL: &str = include_str!("../migrations/sqlite/0001_initial.sql");
const POSTGRES_INITIAL: &str = include_str!("../migrations/postgres/0001_initial.sql");
const SQLITE_AUDIT: &str = include_str!("../migrations/sqlite/0003_audit.sql");
const POSTGRES_AUDIT: &str = include_str!("../migrations/postgres/0003_audit.sql");
const SQLITE_ACCOUNTS: &str = include_str!("../migrations/sqlite/0002_accounts.sql");
const POSTGRES_ACCOUNTS: &str = include_str!("../migrations/postgres/0002_accounts.sql");
const SQLITE_DEVICES_CREDENTIALS: &str =
    include_str!("../migrations/sqlite/0004_devices_credentials.sql");
const POSTGRES_DEVICES_CREDENTIALS: &str =
    include_str!("../migrations/postgres/0004_devices_credentials.sql");

#[derive(Clone, Copy)]
pub enum SessionKind {
    Client,
    Web,
}

impl SessionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Web => "web",
        }
    }
}

#[derive(Debug)]
pub enum AccountError {
    InvalidInput(&'static str),
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict(&'static str),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::Conflict(message) => f.write_str(message),
            Self::Unauthorized => f.write_str("authentication required"),
            Self::Forbidden => f.write_str("operation is forbidden"),
            Self::NotFound => f.write_str("account or session not found"),
        }
    }
}
impl std::error::Error for AccountError {}

#[derive(Serialize)]
pub struct ManagedUser {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub source: String,
    pub disabled: bool,
    pub admitted: bool,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub user_id: String,
    pub kind: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked: bool,
    pub current: bool,
    pub device_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DeviceLogin {
    pub rustdesk_id: String,
    pub uuid: String,
    pub platform: String,
    pub name: String,
    pub device_type: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub rustdesk_id: String,
    pub owner_user_id: Option<String>,
    pub owner_username: Option<String>,
    pub platform: String,
    pub device_name: String,
    pub device_type: String,
    pub alias: String,
    pub last_login_at: i64,
}

#[derive(Clone)]
pub struct Store {
    pool: AnyPool,
    postgres: bool,
    credential_cipher: Option<Arc<CredentialCipher>>,
}

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub source: String,
    pub disabled: bool,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();
        let postgres = url.starts_with("postgres:") || url.starts_with("postgresql:");
        let sqlite = url.starts_with("sqlite:");
        ensure!(
            postgres || sqlite,
            "only SQLite and PostgreSQL are supported"
        );
        // File databases should be created on first boot; respect explicit read-only modes.
        let url = if sqlite && !url.contains(":memory:") && !url.contains("mode=") {
            format!("{url}{}mode=rwc", if url.contains('?') { '&' } else { '?' })
        } else {
            url.to_owned()
        };
        let pool = AnyPoolOptions::new()
            .max_connections(if url.contains(":memory:") { 1 } else { 8 })
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    if sqlite {
                        sqlx::query("PRAGMA foreign_keys = ON")
                            .execute(&mut *connection)
                            .await?;
                        sqlx::query("PRAGMA busy_timeout = 15000")
                            .execute(&mut *connection)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .context("connect account database")?;
        let store = Self {
            pool,
            postgres,
            credential_cipher: None,
        };
        store.migrate().await?;
        Ok(store)
    }

    // SQLx Any uses each driver's native bind syntax, rather than rewriting placeholders.
    fn sql(&self, statement: &str) -> String {
        if !self.postgres {
            return statement.to_owned();
        }
        let mut number = 0;
        statement.chars().fold(String::new(), |mut sql, ch| {
            if ch == '?' {
                number += 1;
                sql.push_str(&format!("${number}"));
            } else {
                sql.push(ch);
            }
            sql
        })
    }

    async fn migrate(&self) -> Result<()> {
        // Create only the migration ledger outside SQLite's transaction, so its
        // first transactional statement can acquire a write lock without first
        // creating a read snapshot (which could fail an upgrade during startup).
        if !self.postgres {
            sqlx::query("CREATE TABLE IF NOT EXISTS account_schema_migrations (version BIGINT PRIMARY KEY NOT NULL)")
                .execute(&self.pool).await?;
        }
        let mut tx = self.pool.begin().await?;
        // The lock also protects first-time schema creation between service processes.
        if self.postgres {
            // Project an integer: PostgreSQL's void return type is unsupported by Any.
            sqlx::query("SELECT 1 FROM pg_advisory_xact_lock(7744196321883)")
                .execute(&mut *tx)
                .await?;
            sqlx::query("CREATE TABLE IF NOT EXISTS account_schema_migrations (version BIGINT PRIMARY KEY NOT NULL)")
                .execute(&mut *tx).await?;
        }
        if !self.postgres {
            // Obtain SQLite's write lock before reading the applied version.
            sqlx::query("UPDATE account_schema_migrations SET version = version")
                .execute(&mut *tx)
                .await?;
        }
        let version: Option<i64> =
            sqlx::query_scalar("SELECT MAX(version) FROM account_schema_migrations")
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            version.unwrap_or(0) <= 4,
            "account database schema is newer than this binary"
        );
        if version.unwrap_or(0) < 1 {
            let migration = if self.postgres {
                POSTGRES_INITIAL
            } else {
                SQLITE_INITIAL
            };
            sqlx::raw_sql(migration).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO account_schema_migrations (version) VALUES (1)")
                .execute(&mut *tx)
                .await?;
        }
        if version.unwrap_or(0) < 2 {
            sqlx::raw_sql(if self.postgres {
                POSTGRES_ACCOUNTS
            } else {
                SQLITE_ACCOUNTS
            })
            .execute(&mut *tx)
            .await?;
            // Backfill under the same migration lock; no database UUID extension is needed.
            let digests: Vec<String> =
                sqlx::query_scalar("SELECT digest FROM sessions WHERE id = ''")
                    .fetch_all(&mut *tx)
                    .await?;
            let now = chrono::Utc::now().timestamp();
            for digest in digests {
                sqlx::query(
                    &self.sql("UPDATE sessions SET id = ?, created_at = ? WHERE digest = ?"),
                )
                .bind(Uuid::new_v4().to_string())
                .bind(now)
                .bind(digest)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("CREATE UNIQUE INDEX sessions_public_id_idx ON sessions(id)")
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO account_schema_migrations (version) VALUES (2)")
                .execute(&mut *tx)
                .await?;
        }
        if version.unwrap_or(0) < 3 {
            sqlx::raw_sql(if self.postgres {
                POSTGRES_AUDIT
            } else {
                SQLITE_AUDIT
            })
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO account_schema_migrations (version) VALUES (3)")
                .execute(&mut *tx)
                .await?;
        }
        if version.unwrap_or(0) < 4 {
            sqlx::raw_sql(if self.postgres {
                POSTGRES_DEVICES_CREDENTIALS
            } else {
                SQLITE_DEVICES_CREDENTIALS
            })
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO account_schema_migrations (version) VALUES (4)")
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Serialize M0 state transitions across pools/processes. This is a database row
    /// lock on PostgreSQL and the first write in a deferred transaction on SQLite.
    async fn write_transaction(&self) -> Result<Transaction<'_, Any>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE settings SET value = value WHERE key = 'policy_version'")
            .execute(&mut *tx)
            .await?;
        Ok(tx)
    }

    async fn audit_tx(&self, tx: &mut Transaction<'_, Any>, event: AuditWrite<'_>) -> Result<()> {
        let policy: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'policy_version'")
                .fetch_one(&mut **tx)
                .await?;
        sqlx::query(&self.sql("INSERT INTO audit_events (timestamp,actor_id,target_id,session_id,source,kind,action,result,reason,before_role,after_role,before_admitted,after_admitted,before_disabled,after_disabled,policy_version,revoked_count) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"))
            .bind(chrono::Utc::now().timestamp()).bind(event.actor_id).bind(event.target_id).bind(event.session_id)
            .bind(event.source.as_str()).bind(event.kind.as_str()).bind(event.action.as_str()).bind(event.result.as_str())
            .bind(event.reason.map(AuditReason::as_str))
            .bind(event.before.map(|s| s.0.as_str())).bind(event.after.map(|s| s.0.as_str()))
            .bind(event.before.map(|s| i64::from(s.1))).bind(event.after.map(|s| i64::from(s.1)))
            .bind(event.before.map(|s| i64::from(s.2))).bind(event.after.map(|s| i64::from(s.2)))
            .bind(policy).bind(event.revoked_count).execute(&mut **tx).await?;
        Ok(())
    }

    async fn account_state_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        user: &User,
    ) -> Result<(AuditRole, bool, bool)> {
        let admitted: i64 =
            sqlx::query_scalar(&self.sql("SELECT admitted FROM users WHERE id = ?"))
                .bind(&user.id)
                .fetch_one(&mut **tx)
                .await?;
        Ok((audit_role(&user.role)?, admitted != 0, user.disabled))
    }

    async fn audit_user_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        actor: Option<&User>,
        user: &User,
        event_type: (AuditKind, AuditAction),
        before: Option<(AuditRole, bool, bool)>,
        revoked_count: i64,
    ) -> Result<()> {
        let source = actor
            .map(|u| audit_source(&u.source))
            .transpose()?
            .unwrap_or(AuditSource::System);
        let mut event = AuditWrite::new(source, event_type.0, event_type.1);
        event.actor_id = actor.map(|u| u.id.as_str());
        event.target_id = Some(&user.id);
        event.before = before;
        event.after = Some(self.account_state_tx(tx, user).await?);
        event.revoked_count = revoked_count;
        self.audit_tx(tx, event).await
    }

    async fn session_failure_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        user_id: &str,
        kind: SessionKind,
        error: &AccountError,
    ) -> Result<()> {
        let row =
            sqlx::query(&self.sql(
                "SELECT id,username,display_name,role,source,disabled FROM users WHERE id = ?",
            ))
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
        let user = row.as_ref().map(read_user).transpose()?;
        let source = user
            .as_ref()
            .map(|u| audit_source(&u.source))
            .transpose()?
            .unwrap_or(AuditSource::System);
        let mut event = AuditWrite::new(source, audit_kind(Some(kind)), AuditAction::Login);
        event.target_id = user.as_ref().map(|u| u.id.as_str());
        event.result = AuditResult::Failure;
        event.reason = Some(if matches!(error, AccountError::InvalidInput(_)) {
            AuditReason::InvalidRequest
        } else {
            AuditReason::SessionDenied
        });
        self.audit_tx(tx, event).await
    }

    pub async fn record_login_failure(
        &self,
        source: &str,
        kind: Option<SessionKind>,
        reason: &str,
    ) -> Result<()> {
        let source =
            AuditSource::parse(source).ok_or(AccountError::InvalidInput("invalid audit source"))?;
        let reason =
            AuditReason::parse(reason).ok_or(AccountError::InvalidInput("invalid audit reason"))?;
        let mut tx = self.write_transaction().await?;
        let mut event = AuditWrite::new(source, audit_kind(kind), AuditAction::Login);
        event.result = AuditResult::Failure;
        event.reason = Some(reason);
        self.audit_tx(&mut tx, event).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn readiness(&self) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM account_schema_migrations")
            .fetch_one(&mut *tx)
            .await?;
        ensure!(version == 4, "account database schema is not ready");
        // Exercise the audit write path too; this probe leaves neither state nor audit rows.
        self.audit_tx(
            &mut tx,
            AuditWrite::new(
                AuditSource::System,
                AuditKind::Unknown,
                AuditAction::PruneAudit,
            ),
        )
        .await?;
        tx.rollback().await?;
        Ok(())
    }

    pub async fn list_audit_events(
        &self,
        actor_web_token: &str,
        filter: &AuditFilter,
    ) -> Result<AuditPage> {
        if !(1..=200).contains(&filter.limit)
            || filter.before_id.is_some_and(|id| id <= 0)
            || matches!((filter.since, filter.until), (Some(s), Some(u)) if s > u)
        {
            return Err(AccountError::InvalidInput("invalid audit filter").into());
        }
        for id in [&filter.actor_id, &filter.target_id].into_iter().flatten() {
            if id.len() > 128 {
                return Err(AccountError::InvalidInput("invalid audit identifier").into());
            }
        }
        let mut tx = self.write_transaction().await?;
        self.actor_tx(&mut tx, actor_web_token, true).await?;
        let mut statement = String::from("SELECT * FROM audit_events WHERE 1=1");
        if filter.before_id.is_some() {
            statement.push_str(" AND id < ?");
        }
        if filter.since.is_some() {
            statement.push_str(" AND timestamp >= ?");
        }
        if filter.until.is_some() {
            statement.push_str(" AND timestamp <= ?");
        }
        for (column, value) in [
            ("actor_id", filter.actor_id.as_deref()),
            ("target_id", filter.target_id.as_deref()),
            ("action", filter.action.map(AuditAction::as_str)),
            ("source", filter.source.map(AuditSource::as_str)),
            ("kind", filter.kind.map(AuditKind::as_str)),
            ("result", filter.result.map(AuditResult::as_str)),
        ] {
            if value.is_some() {
                statement.push_str(&format!(" AND {column} = ?"));
            }
        }
        statement.push_str(" ORDER BY id DESC LIMIT ?");
        let sql = self.sql(&statement);
        let mut query = sqlx::query(&sql);
        if let Some(id) = filter.before_id {
            query = query.bind(id);
        }
        if let Some(since) = filter.since {
            query = query.bind(since);
        }
        if let Some(until) = filter.until {
            query = query.bind(until);
        }
        for value in [
            filter.actor_id.as_deref(),
            filter.target_id.as_deref(),
            filter.action.map(AuditAction::as_str),
            filter.source.map(AuditSource::as_str),
            filter.kind.map(AuditKind::as_str),
            filter.result.map(AuditResult::as_str),
        ]
        .into_iter()
        .flatten()
        {
            query = query.bind(value);
        }
        let rows = query
            .bind(i64::from(filter.limit) + 1)
            .fetch_all(&mut *tx)
            .await?;
        let more = rows.len() > filter.limit as usize;
        let events = rows
            .iter()
            .take(filter.limit as usize)
            .map(read_audit)
            .collect::<Result<Vec<_>>>()?;
        let next_before_id = if more {
            events.last().map(|event| event.id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(AuditPage {
            events,
            next_before_id,
        })
    }

    /// Background/CLI maintenance. Deletes a bounded batch strictly older than the cutoff.
    pub async fn prune_audit(&self, before_timestamp: i64, batch: u32) -> Result<u64> {
        if !(1..=10_000).contains(&batch) || before_timestamp > chrono::Utc::now().timestamp() {
            return Err(AccountError::InvalidInput("invalid audit retention bounds").into());
        }
        let mut tx = self.write_transaction().await?;
        let deleted = sqlx::query(&self.sql("DELETE FROM audit_events WHERE id IN (SELECT id FROM audit_events WHERE timestamp < ? ORDER BY id LIMIT ?)"))
            .bind(before_timestamp).bind(i64::from(batch)).execute(&mut *tx).await?.rows_affected();
        if deleted > 0 {
            let event =
                AuditWrite::new(AuditSource::System, AuditKind::Cli, AuditAction::PruneAudit);
            self.audit_tx(&mut tx, event).await?;
        }
        tx.commit().await?;
        Ok(deleted)
    }

    pub async fn create_local(&self, username: &str, password: &str, role: &str) -> Result<User> {
        validate_credentials(username, password)?;
        validate_role(role)?;
        let hash = hash_password(password).await?;
        let mut tx = self.write_transaction().await?;
        let user = self.insert_local(&mut tx, username, &hash, role).await?;
        let mut event =
            AuditWrite::new(AuditSource::System, AuditKind::Cli, AuditAction::CreateUser);
        event.target_id = Some(&user.id);
        event.after = Some((audit_role(&user.role)?, true, false));
        self.audit_tx(&mut tx, event).await?;
        tx.commit().await?;
        Ok(user)
    }

    pub async fn bootstrap_admin(&self, username: &str, password: &str) -> Result<User> {
        validate_credentials(username, password)?;
        let hash = hash_password(password).await?;
        let mut tx = self.write_transaction().await?;
        let initialized: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'bootstrap_initialized'")
                .fetch_one(&mut *tx)
                .await?;
        if initialized == "1" {
            return Err(
                AccountError::Conflict("administrator bootstrap is already complete").into(),
            );
        }
        let user = self.insert_local(&mut tx, username, &hash, "admin").await?;
        let mut event =
            AuditWrite::new(AuditSource::System, AuditKind::Cli, AuditAction::Bootstrap);
        event.target_id = Some(&user.id);
        event.after = Some((audit_role(&user.role)?, true, false));
        self.audit_tx(&mut tx, event).await?;
        tx.commit().await?;
        Ok(user)
    }

    async fn insert_local(
        &self,
        tx: &mut Transaction<'_, Any>,
        username: &str,
        hash: &str,
        role: &str,
    ) -> Result<User> {
        let user = User {
            id: Uuid::new_v4().to_string(),
            username: username.to_owned(),
            display_name: username.to_owned(),
            role: role.to_owned(),
            source: "local".into(),
            disabled: false,
        };
        sqlx::query(&self.sql("INSERT INTO users (id, username, display_name, role, source, password_hash) VALUES (?, ?, ?, ?, 'local', ?)"))
            .bind(&user.id).bind(username).bind(&user.display_name).bind(role).bind(hash)
            .execute(&mut **tx).await.map_err(account_write_error)?;
        self.create_book(tx, &user.id).await?;
        if role == "admin" {
            sqlx::query("UPDATE settings SET value = '1' WHERE key = 'bootstrap_initialized'")
                .execute(&mut **tx)
                .await?;
        }
        Ok(user)
    }

    async fn local_credentials(&self, username: &str) -> Result<Option<(User, String)>> {
        let row = sqlx::query(&self.sql("SELECT id, username, display_name, role, source, disabled, password_hash FROM users WHERE username = ? AND source = 'local' AND disabled = 0 AND admitted = 1"))
            .bind(username).fetch_optional(&self.pool).await?;
        row.map(|row| Ok((read_user(&row)?, row.try_get("password_hash")?)))
            .transpose()
    }

    pub async fn verify_local(&self, username: &str, password: &str) -> Result<Option<User>> {
        let Some((user, hash)) = self.local_credentials(username).await? else {
            return Ok(None);
        };
        Ok(verify_password(password, &hash).await?.then_some(user))
    }

    /// Verification is expensive and runs outside the lock. Recheck its exact hash
    /// under the write lock so a reset cannot be followed by a stale-password login.
    pub async fn login_local(
        &self,
        username: &str,
        password: &str,
        ttl: i64,
        kind: SessionKind,
    ) -> Result<Option<(User, String)>> {
        self.login_local_with_device(username, password, ttl, kind, None)
            .await
    }

    pub async fn login_local_with_device(
        &self,
        username: &str,
        password: &str,
        ttl: i64,
        kind: SessionKind,
        device: Option<&DeviceLogin>,
    ) -> Result<Option<(User, String)>> {
        let Some((user, hash)) = self.local_credentials(username).await? else {
            self.record_login_failure("local", Some(kind), "invalid_credentials")
                .await?;
            return Ok(None);
        };
        if !verify_password(password, &hash).await? {
            self.record_login_failure("local", Some(kind), "invalid_credentials")
                .await?;
            return Ok(None);
        }
        self.finish_local_login_with_device(&user.id, &hash, ttl, kind, device)
            .await
    }

    #[cfg(test)]
    async fn finish_local_login(
        &self,
        user_id: &str,
        verified_hash: &str,
        ttl: i64,
        kind: SessionKind,
    ) -> Result<Option<(User, String)>> {
        self.finish_local_login_with_device(user_id, verified_hash, ttl, kind, None)
            .await
    }

    async fn finish_local_login_with_device(
        &self,
        user_id: &str,
        verified_hash: &str,
        ttl: i64,
        kind: SessionKind,
        device: Option<&DeviceLogin>,
    ) -> Result<Option<(User, String)>> {
        let mut tx = self.write_transaction().await?;
        let row = sqlx::query(&self.sql("SELECT id, username, display_name, role, source, disabled FROM users WHERE id = ? AND password_hash = ? AND source = 'local' AND disabled = 0 AND admitted = 1 AND role IN ('admin', 'member')"))
            .bind(user_id).bind(verified_hash).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            let mut event = AuditWrite::new(
                AuditSource::Local,
                audit_kind(Some(kind)),
                AuditAction::Login,
            );
            event.result = AuditResult::Failure;
            event.reason = Some(AuditReason::SessionDenied);
            self.audit_tx(&mut tx, event).await?;
            tx.commit().await?;
            return Ok(None);
        };
        let user = read_user(&row)?;
        let token = match self
            .insert_session(&mut tx, user_id, ttl, kind, device)
            .await
        {
            Ok(token) => token,
            Err(error) => {
                if let Some(account_error) = error.downcast_ref::<AccountError>() {
                    self.session_failure_tx(&mut tx, user_id, kind, account_error)
                        .await?;
                    tx.commit().await?;
                }
                return Err(error);
            }
        };
        tx.commit().await?;
        Ok(Some((user, token)))
    }

    pub async fn issue_session(&self, user_id: &str, ttl_seconds: i64) -> Result<String> {
        self.issue_kind_session(user_id, ttl_seconds, SessionKind::Client)
            .await
    }

    pub async fn issue_web_session(&self, user_id: &str, ttl: i64) -> Result<String> {
        self.issue_kind_session(user_id, ttl, SessionKind::Web)
            .await
    }

    async fn issue_kind_session(
        &self,
        user_id: &str,
        ttl: i64,
        kind: SessionKind,
    ) -> Result<String> {
        self.issue_kind_session_with_device(user_id, ttl, kind, None)
            .await
    }

    pub async fn issue_session_with_device(
        &self,
        user_id: &str,
        ttl: i64,
        device: Option<&DeviceLogin>,
    ) -> Result<String> {
        self.issue_kind_session_with_device(user_id, ttl, SessionKind::Client, device)
            .await
    }

    async fn issue_kind_session_with_device(
        &self,
        user_id: &str,
        ttl: i64,
        kind: SessionKind,
        device: Option<&DeviceLogin>,
    ) -> Result<String> {
        let mut tx = self.write_transaction().await?;
        let token = match self
            .insert_session(&mut tx, user_id, ttl, kind, device)
            .await
        {
            Ok(token) => token,
            Err(error) => {
                if let Some(account_error) = error.downcast_ref::<AccountError>() {
                    self.session_failure_tx(&mut tx, user_id, kind, account_error)
                        .await?;
                    tx.commit().await?;
                }
                return Err(error);
            }
        };
        tx.commit().await?;
        Ok(token)
    }

    async fn insert_session(
        &self,
        tx: &mut Transaction<'_, Any>,
        user_id: &str,
        ttl: i64,
        kind: SessionKind,
        device: Option<&DeviceLogin>,
    ) -> Result<String> {
        if ttl <= 0 {
            return Err(AccountError::InvalidInput("session lifetime must be positive").into());
        }
        let now = chrono::Utc::now().timestamp();
        let expires = now
            .checked_add(ttl)
            .ok_or(AccountError::InvalidInput("session expiry overflow"))?;
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        let session_id = Uuid::new_v4().to_string();
        let inserted = sqlx::query(&self.sql("INSERT INTO sessions (digest, id, user_id, kind, created_at, expires_at) SELECT ?, ?, id, ?, ?, ? FROM users WHERE id = ? AND disabled = 0 AND admitted = 1 AND role IN ('admin', 'member')"))
            .bind(token_digest(&token)).bind(&session_id).bind(kind.as_str()).bind(now).bind(expires).bind(user_id)
            .execute(&mut **tx).await?;
        if inserted.rows_affected() != 1 {
            return Err(AccountError::Unauthorized.into());
        }
        if matches!(kind, SessionKind::Client)
            && let Some(device) = device
            && let Some(device_id) = self.associate_device_tx(tx, user_id, device, now).await?
        {
            sqlx::query(&self.sql("UPDATE sessions SET device_id = ? WHERE id = ?"))
                .bind(device_id)
                .bind(&session_id)
                .execute(&mut **tx)
                .await?;
        }
        let user = self.user_tx(tx, user_id).await?;
        let mut event = AuditWrite::new(
            audit_source(&user.source)?,
            audit_kind(Some(kind)),
            AuditAction::Login,
        );
        event.actor_id = Some(user_id);
        event.target_id = Some(user_id);
        event.session_id = Some(&session_id);
        self.audit_tx(tx, event).await?;
        Ok(token)
    }

    pub async fn authenticate(&self, token: &str) -> Result<Option<User>> {
        self.authenticate_kind(token, SessionKind::Client).await
    }

    pub async fn authenticate_web(&self, token: &str) -> Result<Option<User>> {
        self.authenticate_kind(token, SessionKind::Web).await
    }

    async fn authenticate_kind(&self, token: &str, kind: SessionKind) -> Result<Option<User>> {
        if !valid_token(token) {
            return Ok(None);
        }
        let row = sqlx::query(&self.sql("SELECT u.id, u.username, u.display_name, u.role, u.source, u.disabled FROM users u JOIN sessions s ON s.user_id = u.id WHERE s.digest = ? AND s.kind = ? AND s.revoked = 0 AND s.expires_at > ? AND u.disabled = 0 AND u.admitted = 1 AND u.role IN ('admin', 'member')"))
            .bind(token_digest(token)).bind(kind.as_str()).bind(chrono::Utc::now().timestamp())
            .fetch_optional(&self.pool).await?;
        row.as_ref().map(read_user).transpose()
    }

    /// Internal/system revocation, deliberately distinct from a user logout.
    pub async fn revoke_session(&self, token: &str) -> Result<()> {
        self.revoke_token(token, None).await
    }

    pub async fn logout_session(&self, token: &str, kind: SessionKind) -> Result<()> {
        self.revoke_token(token, Some(kind)).await
    }

    async fn revoke_token(&self, token: &str, logout_kind: Option<SessionKind>) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        if let Some(kind) = logout_kind {
            self.authenticated_kind_tx(&mut tx, token, kind).await?;
        }
        let row = sqlx::query(&self.sql("SELECT s.id,s.user_id,s.kind,u.source FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.digest = ?"))
            .bind(token_digest(token)).fetch_optional(&mut *tx).await?;
        if let Some(row) = row {
            let id: String = row.try_get("id")?;
            let user_id: String = row.try_get("user_id")?;
            let kind: String = row.try_get("kind")?;
            let source: String = row.try_get("source")?;
            let revoked = sqlx::query(
                &self.sql("UPDATE sessions SET revoked = 1 WHERE digest = ? AND revoked = 0"),
            )
            .bind(token_digest(token))
            .execute(&mut *tx)
            .await?
            .rows_affected();
            let mut event = AuditWrite::new(
                if logout_kind.is_some() {
                    audit_source(&source)?
                } else {
                    AuditSource::System
                },
                if logout_kind.is_some() {
                    AuditKind::parse(&kind).context("invalid stored session kind")?
                } else {
                    AuditKind::Unknown
                },
                if logout_kind.is_some() {
                    AuditAction::Logout
                } else {
                    AuditAction::RevokeSession
                },
            );
            if logout_kind.is_some() {
                event.actor_id = Some(&user_id);
            }
            event.target_id = Some(&user_id);
            event.session_id = Some(&id);
            event.revoked_count = i64::try_from(revoked)?;
            self.audit_tx(&mut tx, event).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn authenticated_kind_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        token: &str,
        kind: SessionKind,
    ) -> Result<User> {
        if !valid_token(token) {
            return Err(AccountError::Unauthorized.into());
        }
        let row = sqlx::query(&self.sql("SELECT u.id,u.username,u.display_name,u.role,u.source,u.disabled FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.digest = ? AND s.kind = ? AND s.revoked = 0 AND s.expires_at > ? AND u.disabled = 0 AND u.admitted = 1 AND u.role IN ('admin','member')"))
            .bind(token_digest(token)).bind(kind.as_str()).bind(chrono::Utc::now().timestamp())
            .fetch_optional(&mut **tx).await?;
        row.as_ref()
            .map(read_user)
            .transpose()?
            .ok_or_else(|| AccountError::Unauthorized.into())
    }

    pub async fn find_or_create_oidc(
        &self,
        issuer: &str,
        subject: &str,
        display_name: &str,
        role: Option<&str>,
    ) -> Result<Option<User>> {
        if issuer.is_empty() || subject.is_empty() {
            self.record_login_failure("oidc", None, "invalid_request")
                .await?;
            return Err(AccountError::InvalidInput("OIDC identity is required").into());
        }
        if let Some(role) = role
            && let Err(error) = validate_role(role)
        {
            self.record_login_failure("oidc", None, "invalid_request")
                .await?;
            return Err(error);
        }
        let mut tx = self.write_transaction().await?;
        let existing = sqlx::query(&self.sql("SELECT id, username, display_name, role, source, disabled FROM users WHERE issuer = ? AND subject = ?"))
            .bind(issuer).bind(subject).fetch_optional(&mut *tx).await?;
        let created = existing.is_none();
        let mut user = if let Some(row) = existing {
            read_user(&row)?
        } else {
            let id = Uuid::new_v4().to_string();
            let user = User {
                username: format!("oidc-{id}"),
                id,
                display_name: display_name.to_owned(),
                role: role.unwrap_or("").to_owned(),
                source: "oidc".into(),
                disabled: false,
            };
            sqlx::query(&self.sql("INSERT INTO users (id, username, display_name, role, source, admitted, issuer, subject) VALUES (?, ?, ?, ?, 'oidc', ?, ?, ?)"))
                .bind(&user.id).bind(&user.username).bind(display_name).bind(&user.role)
                .bind(i64::from(role.is_some())).bind(issuer).bind(subject).execute(&mut *tx).await?;
            self.create_book(&mut tx, &user.id).await?;
            user
        };
        let before = if created {
            None
        } else {
            Some(self.account_state_tx(&mut tx, &user).await?)
        };
        // A policy match must never re-enable a user disabled by an administrator.
        let admitted = role.is_some() && !user.disabled;
        let next_role = if admitted { role.unwrap_or("") } else { "" };
        let revoked_count = if !admitted || (user.role == "admin" && next_role == "member") {
            self.revoke_sessions_tx(&mut tx, &user.id).await?
        } else {
            0
        };
        sqlx::query(
            &self.sql("UPDATE users SET display_name = ?, role = ?, admitted = ? WHERE id = ?"),
        )
        .bind(display_name)
        .bind(next_role)
        .bind(i64::from(admitted))
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
        user.display_name = display_name.to_owned();
        user.role = next_role.to_owned();
        let mut event = AuditWrite::new(
            AuditSource::Oidc,
            AuditKind::Unknown,
            if created {
                AuditAction::OidcJit
            } else {
                AuditAction::OidcMapping
            },
        );
        event.target_id = Some(&user.id);
        event.before = before;
        event.after = Some((audit_role(&user.role)?, admitted, user.disabled));
        event.revoked_count = revoked_count;
        self.audit_tx(&mut tx, event).await?;
        if !admitted {
            let mut failure =
                AuditWrite::new(AuditSource::Oidc, AuditKind::Unknown, AuditAction::Login);
            failure.target_id = Some(&user.id);
            failure.result = AuditResult::Failure;
            failure.reason = Some(AuditReason::SessionDenied);
            self.audit_tx(&mut tx, failure).await?;
        }
        tx.commit().await?;
        Ok(admitted.then_some(user))
    }

    pub async fn sync_policy_version(&self, version: &str) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        let current: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'policy_version'")
                .fetch_one(&mut *tx)
                .await?;
        if current != version {
            sqlx::query(&self.sql("UPDATE settings SET value = ? WHERE key = 'policy_version'"))
                .bind(version)
                .execute(&mut *tx)
                .await?;
            let revoked_count = sqlx::query("UPDATE sessions SET revoked = 1 WHERE revoked = 0 AND user_id IN (SELECT id FROM users WHERE source = 'oidc')")
                .execute(&mut *tx).await?.rows_affected();
            let mut event = AuditWrite::new(
                AuditSource::System,
                AuditKind::Unknown,
                AuditAction::PolicyChange,
            );
            event.revoked_count = i64::try_from(revoked_count)?;
            self.audit_tx(&mut tx, event).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Login material is self-reported. It associates inventory only; it never
    /// proves device identity or grants remote-control permission.
    async fn associate_device_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        user_id: &str,
        device: &DeviceLogin,
        now: i64,
    ) -> Result<Option<String>> {
        let bounded = |value: &str, maximum: usize| {
            value.len() <= maximum && !value.chars().any(char::is_control)
        };
        if device.rustdesk_id.trim().is_empty()
            || device.uuid.trim().is_empty()
            || !bounded(&device.rustdesk_id, 128)
            || !bounded(&device.uuid, 256)
            || !bounded(&device.platform, 128)
            || !bounded(&device.name, 256)
            || !bounded(&device.device_type, 128)
        {
            return Ok(None);
        }
        let rows = sqlx::query(&self.sql("SELECT id,rustdesk_id,uuid,owner_user_id FROM devices WHERE rustdesk_id = ? OR uuid = ?"))
            .bind(&device.rustdesk_id).bind(&device.uuid).fetch_all(&mut **tx).await?;
        let id = if rows.is_empty() {
            let id = Uuid::new_v4().to_string();
            sqlx::query(&self.sql("INSERT INTO devices (id,rustdesk_id,uuid,owner_user_id,platform,name,device_type,last_login_at) VALUES (?,?,?,?,?,?,?,?)"))
                .bind(&id).bind(&device.rustdesk_id).bind(&device.uuid).bind(user_id)
                .bind(&device.platform).bind(&device.name).bind(&device.device_type).bind(now)
                .execute(&mut **tx).await?;
            return Ok(Some(id));
        } else if rows.len() == 1 {
            let row = &rows[0];
            let uuid: String = row.try_get("uuid")?;
            let owner: Option<String> = row.try_get("owner_user_id")?;
            let rustdesk_id: String = row.try_get("rustdesk_id")?;
            if uuid != device.uuid
                || owner.as_deref().is_some_and(|owner| owner != user_id)
                || (rustdesk_id != device.rustdesk_id && owner.as_deref() != Some(user_id))
            {
                return Ok(None);
            }
            row.try_get::<String, _>("id")?
        } else {
            return Ok(None);
        };
        sqlx::query(&self.sql("UPDATE devices SET rustdesk_id = ?,platform = CASE WHEN ? = '' THEN platform ELSE ? END,name = CASE WHEN ? = '' THEN name ELSE ? END,device_type = CASE WHEN ? = '' THEN device_type ELSE ? END,last_login_at = ? WHERE id = ?"))
            .bind(&device.rustdesk_id).bind(&device.platform).bind(&device.platform).bind(&device.name).bind(&device.name).bind(&device.device_type).bind(&device.device_type).bind(now).bind(&id)
            .execute(&mut **tx).await?;
        Ok(Some(id))
    }

    pub async fn list_devices(
        &self,
        token: &str,
        kind: SessionKind,
        all: bool,
    ) -> Result<Vec<DeviceInfo>> {
        let mut tx = self.write_transaction().await?;
        let actor = self.authenticated_kind_tx(&mut tx, token, kind).await?;
        if all && (!matches!(kind, SessionKind::Web) || actor.role != "admin") {
            return Err(AccountError::Forbidden.into());
        }
        let sql = self.sql(if all {
            "SELECT d.id,d.rustdesk_id,d.owner_user_id,u.username AS owner_username,d.platform,d.name,d.device_type,d.alias,d.last_login_at FROM devices d LEFT JOIN users u ON u.id=d.owner_user_id ORDER BY d.last_login_at DESC,d.id"
        } else {
            "SELECT d.id,d.rustdesk_id,d.owner_user_id,u.username AS owner_username,d.platform,d.name,d.device_type,d.alias,d.last_login_at FROM devices d LEFT JOIN users u ON u.id=d.owner_user_id WHERE d.owner_user_id = ? ORDER BY d.last_login_at DESC,d.id"
        });
        let mut query = sqlx::query(&sql);
        if !all {
            query = query.bind(&actor.id);
        }
        let devices = query
            .fetch_all(&mut *tx)
            .await?
            .iter()
            .map(|row| {
                Ok(DeviceInfo {
                    id: row.try_get("id")?,
                    rustdesk_id: row.try_get("rustdesk_id")?,
                    owner_user_id: row.try_get("owner_user_id")?,
                    owner_username: row.try_get("owner_username")?,
                    platform: row.try_get("platform")?,
                    device_name: row.try_get("name")?,
                    device_type: row.try_get("device_type")?,
                    alias: row.try_get("alias")?,
                    last_login_at: row.try_get("last_login_at")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(devices)
    }

    pub async fn assign_device(
        &self,
        actor_web_token: &str,
        device_id: &str,
        owner_user_id: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_web_token, true).await?;
        let old_owner: Option<String> =
            sqlx::query_scalar(&self.sql("SELECT owner_user_id FROM devices WHERE id = ?"))
                .bind(device_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(AccountError::NotFound)?;
        if let Some(owner) = owner_user_id {
            self.user_tx(&mut tx, owner).await?;
        }
        sqlx::query(&self.sql("UPDATE devices SET owner_user_id = ? WHERE id = ?"))
            .bind(owner_user_id)
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        let revoked = sqlx::query(&self.sql("UPDATE sessions SET revoked = 1 WHERE device_id = ? AND kind = 'client' AND revoked = 0"))
            .bind(device_id).execute(&mut *tx).await?.rows_affected();
        sqlx::query(&self.sql("INSERT INTO device_events (timestamp,actor_id,device_id,action,old_owner_id,new_owner_id,revoked_count) VALUES (?,?,?,?,?,?,?)"))
            .bind(chrono::Utc::now().timestamp()).bind(&actor.id).bind(device_id)
            .bind(if owner_user_id.is_some() { "assign" } else { "unassign" })
            .bind(old_owner).bind(owner_user_id).bind(i64::try_from(revoked)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_device_alias(
        &self,
        actor_web_token: &str,
        device_id: &str,
        alias: &str,
    ) -> Result<()> {
        if alias.len() > 256 || alias.chars().any(char::is_control) {
            return Err(AccountError::InvalidInput("invalid device alias").into());
        }
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_web_token, true).await?;
        let owner: Option<String> =
            sqlx::query_scalar(&self.sql("SELECT owner_user_id FROM devices WHERE id = ?"))
                .bind(device_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(AccountError::NotFound)?;
        sqlx::query(&self.sql("UPDATE devices SET alias = ? WHERE id = ?"))
            .bind(alias)
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&self.sql("INSERT INTO device_events (timestamp,actor_id,device_id,action,old_owner_id,new_owner_id,revoked_count) VALUES (?,?,?,'alias',?,?,0)"))
            .bind(chrono::Utc::now().timestamp()).bind(&actor.id).bind(device_id).bind(&owner).bind(&owner).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub fn credential_sync_enabled(&self) -> bool {
        self.credential_cipher.is_some()
    }

    /// Validate the entire existing key inventory before accepting traffic.
    pub async fn configure_credential_cipher(
        mut self,
        cipher: Option<Arc<CredentialCipher>>,
    ) -> Result<Self> {
        let mut tx = self.write_transaction().await?;
        let rows =
            sqlx::query("SELECT user_id,peer_id,key_id,nonce,ciphertext FROM book_credentials")
                .fetch_all(&mut *tx)
                .await?;
        for row in rows {
            let cipher = cipher
                .as_ref()
                .context("credential key ring is required for stored credentials")?;
            let owner: String = row.try_get("user_id")?;
            let peer: String = row.try_get("peer_id")?;
            cipher.open(&owner, &peer, &read_sealed_credential(&row)?)?;
        }
        tx.commit().await?;
        self.credential_cipher = cipher;
        Ok(self)
    }

    async fn get_book_tx(&self, tx: &mut Transaction<'_, Any>, user_id: &str) -> Result<Value> {
        let snapshot: String =
            sqlx::query_scalar(&self.sql("SELECT snapshot FROM books WHERE user_id = ?"))
                .bind(user_id)
                .fetch_one(&mut **tx)
                .await?;
        let mut value: Value =
            serde_json::from_str(&snapshot).context("invalid stored address book")?;
        strip_book_secrets(&mut value);
        let rows =
            sqlx::query(&self.sql(
                "SELECT peer_id,key_id,nonce,ciphertext FROM book_credentials WHERE user_id = ?",
            ))
            .bind(user_id)
            .fetch_all(&mut **tx)
            .await?;
        for row in rows {
            let cipher = self
                .credential_cipher
                .as_ref()
                .context("credential key ring is required for stored credentials")?;
            let peer_id: String = row.try_get("peer_id")?;
            let hash = cipher.open(user_id, &peer_id, &read_sealed_credential(&row)?)?;
            if let Some(peers) = value.get_mut("peers").and_then(Value::as_array_mut) {
                for peer in peers {
                    if peer.get("id").and_then(Value::as_str) == Some(peer_id.as_str())
                        && let Some(object) = peer.as_object_mut()
                    {
                        object.insert("hash".to_owned(), Value::String(hash.clone()));
                    }
                }
            }
        }
        Ok(value)
    }

    async fn put_book_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        user_id: &str,
        snapshot: &Value,
    ) -> Result<()> {
        // Fail closed even for an empty snapshot: metadata-only processes must not
        // silently erase another process's encrypted credentials.
        let rows =
            sqlx::query(&self.sql(
                "SELECT peer_id,key_id,nonce,ciphertext FROM book_credentials WHERE user_id = ?",
            ))
            .bind(user_id)
            .fetch_all(&mut **tx)
            .await?;
        for row in rows {
            let cipher = self
                .credential_cipher
                .as_ref()
                .context("credential key ring is required for stored credentials")?;
            let peer_id: String = row.try_get("peer_id")?;
            cipher.open(user_id, &peer_id, &read_sealed_credential(&row)?)?;
        }
        let mut sealed = Vec::new();
        if let Some(cipher) = &self.credential_cipher {
            let mut ids = std::collections::HashSet::new();
            if let Some(peers) = snapshot.get("peers").and_then(Value::as_array) {
                ensure!(peers.len() <= 1000, "too many address book peers");
                for peer in peers {
                    let id = peer
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty() && id.len() <= 256)
                        .ok_or(AccountError::InvalidInput("invalid peer identifier"))?;
                    ensure!(ids.insert(id), "duplicate peer identifier");
                    if let Some(hash) = peer.get("hash") {
                        let hash = hash
                            .as_str()
                            .ok_or(AccountError::InvalidInput("invalid credential hash"))?;
                        if !hash.is_empty() {
                            sealed.push((id.to_owned(), cipher.seal(user_id, id, hash)?));
                        }
                    }
                }
            }
        }
        let mut metadata = snapshot.clone();
        strip_book_secrets(&mut metadata);
        let changed = sqlx::query(&self.sql("UPDATE books SET snapshot = ? WHERE user_id = ?"))
            .bind(serde_json::to_string(&metadata)?)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        ensure!(
            changed.rows_affected() == 1,
            "address book owner does not exist"
        );
        sqlx::query(&self.sql("DELETE FROM book_credentials WHERE user_id = ?"))
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        for (peer_id, secret) in sealed {
            sqlx::query(&self.sql("INSERT INTO book_credentials (user_id,peer_id,key_id,nonce,ciphertext) VALUES (?,?,?,?,?)"))
                .bind(user_id).bind(peer_id).bind(secret.key_id).bind(secret.nonce).bind(secret.ciphertext)
                .execute(&mut **tx).await?;
        }
        Ok(())
    }

    pub async fn get_book(&self, user_id: &str) -> Result<Value> {
        let mut tx = self.write_transaction().await?;
        let value = self.get_book_tx(&mut tx, user_id).await?;
        tx.commit().await?;
        Ok(value)
    }

    pub async fn put_book(&self, user_id: &str, snapshot: &Value) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        self.put_book_tx(&mut tx, user_id, snapshot).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_book_authenticated(&self, client_token: &str) -> Result<Value> {
        let mut tx = self.write_transaction().await?;
        let actor = self
            .authenticated_kind_tx(&mut tx, client_token, SessionKind::Client)
            .await?;
        let value = self.get_book_tx(&mut tx, &actor.id).await?;
        tx.commit().await?;
        Ok(value)
    }

    pub async fn put_book_authenticated(&self, client_token: &str, snapshot: &Value) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        let actor = self
            .authenticated_kind_tx(&mut tx, client_token, SessionKind::Client)
            .await?;
        self.put_book_tx(&mut tx, &actor.id, snapshot).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn disable_user(&self, user_id: &str) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        let mut user = self.user_tx(&mut tx, user_id).await?;
        let before = self.account_state_tx(&mut tx, &user).await?;
        self.protect_last_admin(&mut tx, &user).await?;
        sqlx::query(&self.sql("UPDATE users SET disabled = 1, admitted = 0 WHERE id = ?"))
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        let revoked = self.revoke_sessions_tx(&mut tx, user_id).await?;
        user.disabled = true;
        self.audit_user_tx(
            &mut tx,
            None,
            &user,
            (AuditKind::Cli, AuditAction::UpdateUser),
            Some(before),
            revoked,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn actor_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        token: &str,
        admin: bool,
    ) -> Result<User> {
        let actor = self
            .authenticated_kind_tx(tx, token, SessionKind::Web)
            .await?;
        if admin && actor.role != "admin" {
            return Err(AccountError::Forbidden.into());
        }
        Ok(actor)
    }

    async fn user_tx(&self, tx: &mut Transaction<'_, Any>, id: &str) -> Result<User> {
        let row = sqlx::query(&self.sql(
            "SELECT id, username, display_name, role, source, disabled FROM users WHERE id = ?",
        ))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        row.as_ref()
            .map(read_user)
            .transpose()?
            .ok_or_else(|| AccountError::NotFound.into())
    }

    async fn protect_last_admin(&self, tx: &mut Transaction<'_, Any>, user: &User) -> Result<()> {
        if user.source == "local" && user.role == "admin" && !user.disabled {
            let admitted: i64 =
                sqlx::query_scalar(&self.sql("SELECT admitted FROM users WHERE id = ?"))
                    .bind(&user.id)
                    .fetch_one(&mut **tx)
                    .await?;
            if admitted == 0 {
                return Ok(());
            }
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE source = 'local' AND role = 'admin' AND disabled = 0 AND admitted = 1")
                .fetch_one(&mut **tx).await?;
            if count <= 1 {
                return Err(AccountError::Conflict(
                    "cannot disable or demote the last local administrator",
                )
                .into());
            }
        }
        Ok(())
    }

    pub async fn admin_create_local(
        &self,
        actor_token: &str,
        username: &str,
        password: &str,
        role: &str,
    ) -> Result<User> {
        validate_credentials(username, password)?;
        validate_role(role)?;
        let hash = hash_password(password).await?;
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_token, true).await?;
        let user = self.insert_local(&mut tx, username, &hash, role).await?;
        self.audit_user_tx(
            &mut tx,
            Some(&actor),
            &user,
            (AuditKind::Web, AuditAction::CreateUser),
            None,
            0,
        )
        .await?;
        tx.commit().await?;
        Ok(user)
    }

    pub async fn admin_update_user(
        &self,
        actor_token: &str,
        target_id: &str,
        disabled: Option<bool>,
        role: Option<&str>,
    ) -> Result<User> {
        if let Some(role) = role {
            validate_role(role)?;
        }
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_token, true).await?;
        let mut user = self.user_tx(&mut tx, target_id).await?;
        let before = self.account_state_tx(&mut tx, &user).await?;
        if role.is_some() && user.source != "local" {
            return Err(
                AccountError::Conflict("OIDC roles are managed by admission policy").into(),
            );
        }
        if disabled == Some(true) || role.is_some_and(|role| role != "admin") {
            self.protect_last_admin(&mut tx, &user).await?;
        }
        let next_disabled = disabled.unwrap_or(user.disabled);
        let next_role = role.unwrap_or(&user.role).to_owned();
        // Re-enabled OIDC identities must pass admission policy on their next login.
        if let Some(disabled) = disabled {
            sqlx::query(&self.sql("UPDATE users SET disabled = ?, admitted = ? WHERE id = ?"))
                .bind(i64::from(disabled))
                .bind(i64::from(!disabled && user.source == "local"))
                .bind(target_id)
                .execute(&mut *tx)
                .await?;
        }
        if role == Some("admin") {
            sqlx::query("UPDATE settings SET value = '1' WHERE key = 'bootstrap_initialized'")
                .execute(&mut *tx)
                .await?;
        }
        if role.is_some() {
            sqlx::query(&self.sql("UPDATE users SET role = ? WHERE id = ?"))
                .bind(&next_role)
                .bind(target_id)
                .execute(&mut *tx)
                .await?;
        }
        let revoked = if next_disabled
            || (user.role == "admin" && next_role != "admin")
            || (user.source == "oidc" && disabled.is_some())
        {
            self.revoke_sessions_tx(&mut tx, target_id).await?
        } else {
            0
        };
        user.disabled = next_disabled;
        user.role = next_role;
        self.audit_user_tx(
            &mut tx,
            Some(&actor),
            &user,
            (AuditKind::Web, AuditAction::UpdateUser),
            Some(before),
            revoked,
        )
        .await?;
        tx.commit().await?;
        Ok(user)
    }

    pub async fn admin_reset_password(
        &self,
        actor_token: &str,
        target_id: &str,
        new_password: &str,
    ) -> Result<()> {
        validate_password(new_password)?;
        let hash = hash_password(new_password).await?;
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_token, true).await?;
        let user = self.user_tx(&mut tx, target_id).await?;
        let before = self.account_state_tx(&mut tx, &user).await?;
        if user.source != "local" {
            return Err(AccountError::Conflict(
                "OIDC passwords are managed by the identity provider",
            )
            .into());
        }
        sqlx::query(&self.sql("UPDATE users SET password_hash = ? WHERE id = ?"))
            .bind(hash)
            .bind(target_id)
            .execute(&mut *tx)
            .await?;
        let revoked = self.revoke_sessions_tx(&mut tx, target_id).await?;
        self.audit_user_tx(
            &mut tx,
            Some(&actor),
            &user,
            (AuditKind::Web, AuditAction::ResetPassword),
            Some(before),
            revoked,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn change_password(
        &self,
        actor_token: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<()> {
        validate_password(new_password)?;
        let actor = self
            .authenticate_web(actor_token)
            .await?
            .ok_or(AccountError::Unauthorized)?;
        if actor.source != "local" {
            return Err(AccountError::Conflict(
                "OIDC passwords are managed by the identity provider",
            )
            .into());
        }
        let old_hash: String =
            sqlx::query_scalar(&self.sql("SELECT password_hash FROM users WHERE id = ?"))
                .bind(&actor.id)
                .fetch_one(&self.pool)
                .await?;
        if !verify_password(current_password, &old_hash).await? {
            return Err(AccountError::Unauthorized.into());
        }
        let new_hash = hash_password(new_password).await?;
        let mut tx = self.write_transaction().await?;
        let current_actor = self.actor_tx(&mut tx, actor_token, false).await?;
        if current_actor.id != actor.id || current_actor.source != "local" {
            return Err(AccountError::Forbidden.into());
        }
        let changed = sqlx::query(
            &self.sql("UPDATE users SET password_hash = ? WHERE id = ? AND password_hash = ?"),
        )
        .bind(new_hash)
        .bind(&actor.id)
        .bind(old_hash)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(AccountError::Conflict("password changed concurrently").into());
        }
        let before = self.account_state_tx(&mut tx, &current_actor).await?;
        let revoked = self.revoke_sessions_tx(&mut tx, &actor.id).await?;
        self.audit_user_tx(
            &mut tx,
            Some(&current_actor),
            &current_actor,
            (AuditKind::Web, AuditAction::ChangePassword),
            Some(before),
            revoked,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_users(&self, actor_token: &str) -> Result<Vec<ManagedUser>> {
        let mut tx = self.write_transaction().await?;
        self.actor_tx(&mut tx, actor_token, true).await?;
        let rows = sqlx::query("SELECT id, username, display_name, role, source, disabled, admitted FROM users ORDER BY username, id")
            .fetch_all(&mut *tx).await?;
        let users = rows
            .iter()
            .map(|row| {
                Ok(ManagedUser {
                    id: row.try_get("id")?,
                    username: row.try_get("username")?,
                    display_name: row.try_get("display_name")?,
                    role: row.try_get("role")?,
                    source: row.try_get("source")?,
                    disabled: row.try_get::<i64, _>("disabled")? != 0,
                    admitted: row.try_get::<i64, _>("admitted")? != 0,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(users)
    }

    fn managed_target<'a>(&self, actor: &'a User, target: Option<&'a str>) -> Result<&'a str> {
        let target = target.unwrap_or(&actor.id);
        if target != actor.id && actor.role != "admin" {
            return Err(AccountError::Forbidden.into());
        }
        Ok(target)
    }

    pub async fn list_sessions(
        &self,
        actor_token: &str,
        target_user_id: Option<&str>,
    ) -> Result<Vec<SessionInfo>> {
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_token, false).await?;
        let target = self.managed_target(&actor, target_user_id)?;
        self.user_tx(&mut tx, target).await?;
        let rows = sqlx::query(&self.sql("SELECT id, user_id, kind, created_at, expires_at, revoked, digest, device_id FROM sessions WHERE user_id = ? ORDER BY created_at, id"))
            .bind(target).fetch_all(&mut *tx).await?;
        let digest = token_digest(actor_token);
        let sessions = rows
            .iter()
            .map(|row| {
                Ok(SessionInfo {
                    id: row.try_get("id")?,
                    user_id: row.try_get("user_id")?,
                    kind: row.try_get("kind")?,
                    created_at: row.try_get("created_at")?,
                    expires_at: row.try_get("expires_at")?,
                    revoked: row.try_get::<i64, _>("revoked")? != 0,
                    current: row.try_get::<String, _>("digest")? == digest,
                    device_id: row.try_get("device_id")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(sessions)
    }

    pub async fn revoke_managed_session(&self, actor_token: &str, session_id: &str) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_token, false).await?;
        let target: String =
            sqlx::query_scalar(&self.sql("SELECT user_id FROM sessions WHERE id = ?"))
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(AccountError::NotFound)?;
        self.managed_target(&actor, Some(&target))?;
        let revoked =
            sqlx::query(&self.sql("UPDATE sessions SET revoked = 1 WHERE id = ? AND revoked = 0"))
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
        self.user_tx(&mut tx, &target).await?;
        let mut event = AuditWrite::new(
            audit_source(&actor.source)?,
            AuditKind::Web,
            AuditAction::RevokeSession,
        );
        event.actor_id = Some(&actor.id);
        event.target_id = Some(&target);
        event.session_id = Some(session_id);
        event.revoked_count = i64::try_from(revoked.rows_affected())?;
        self.audit_tx(&mut tx, event).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn revoke_user_sessions(
        &self,
        actor_token: &str,
        target_user_id: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.write_transaction().await?;
        let actor = self.actor_tx(&mut tx, actor_token, false).await?;
        let target = self.managed_target(&actor, target_user_id)?;
        self.user_tx(&mut tx, target).await?;
        let revoked = self.revoke_sessions_tx(&mut tx, target).await?;
        let mut event = AuditWrite::new(
            audit_source(&actor.source)?,
            AuditKind::Web,
            AuditAction::RevokeAll,
        );
        event.actor_id = Some(&actor.id);
        event.target_id = Some(target);
        event.revoked_count = revoked;
        self.audit_tx(&mut tx, event).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn create_book(&self, tx: &mut Transaction<'_, Any>, user_id: &str) -> Result<()> {
        sqlx::query(&self.sql("INSERT INTO books (user_id, snapshot) VALUES (?, ?)"))
            .bind(user_id)
            .bind(r#"{"peers":[],"tags":[]}"#)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn revoke_sessions_tx(
        &self,
        tx: &mut Transaction<'_, Any>,
        user_id: &str,
    ) -> Result<i64> {
        let changed = sqlx::query(
            &self.sql("UPDATE sessions SET revoked = 1 WHERE user_id = ? AND revoked = 0"),
        )
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
        Ok(i64::try_from(changed.rows_affected())?)
    }
}

fn strip_book_secrets(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("hash");
            object.remove("password");
            for value in object.values_mut() {
                strip_book_secrets(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                strip_book_secrets(value);
            }
        }
        _ => {}
    }
}

fn read_sealed_credential(row: &sqlx::any::AnyRow) -> Result<SealedCredential> {
    Ok(SealedCredential {
        key_id: row.try_get("key_id")?,
        nonce: row.try_get("nonce")?,
        ciphertext: row.try_get("ciphertext")?,
    })
}

fn audit_source(value: &str) -> Result<AuditSource> {
    AuditSource::parse(value).ok_or_else(|| anyhow::anyhow!("invalid stored account source"))
}
fn audit_kind(kind: Option<SessionKind>) -> AuditKind {
    match kind {
        Some(SessionKind::Client) => AuditKind::Client,
        Some(SessionKind::Web) => AuditKind::Web,
        None => AuditKind::Unknown,
    }
}
fn audit_role(value: &str) -> Result<AuditRole> {
    match value {
        "" => Ok(AuditRole::None),
        "admin" => Ok(AuditRole::Admin),
        "member" => Ok(AuditRole::Member),
        _ => anyhow::bail!("invalid stored account role"),
    }
}
fn read_audit(row: &sqlx::any::AnyRow) -> Result<AuditEvent> {
    let role = |column| -> Result<Option<AuditRole>> {
        row.try_get::<Option<String>, _>(column)?
            .map(|s| AuditRole::parse(&s).context("invalid audit role"))
            .transpose()
    };
    let boolean = |column| -> Result<Option<bool>> {
        Ok(row.try_get::<Option<i64>, _>(column)?.map(|v| v != 0))
    };
    Ok(AuditEvent {
        id: row.try_get("id")?,
        timestamp: row.try_get("timestamp")?,
        actor_id: row.try_get("actor_id")?,
        target_id: row.try_get("target_id")?,
        session_id: row.try_get("session_id")?,
        source: AuditSource::parse(&row.try_get::<String, _>("source")?)
            .context("invalid audit source")?,
        kind: AuditKind::parse(&row.try_get::<String, _>("kind")?).context("invalid audit kind")?,
        action: AuditAction::parse(&row.try_get::<String, _>("action")?)
            .context("invalid audit action")?,
        result: AuditResult::parse(&row.try_get::<String, _>("result")?)
            .context("invalid audit result")?,
        reason: row
            .try_get::<Option<String>, _>("reason")?
            .map(|s| AuditReason::parse(&s).context("invalid audit reason"))
            .transpose()?,
        before_role: role("before_role")?,
        after_role: role("after_role")?,
        before_admitted: boolean("before_admitted")?,
        after_admitted: boolean("after_admitted")?,
        before_disabled: boolean("before_disabled")?,
        after_disabled: boolean("after_disabled")?,
        policy_version: row.try_get("policy_version")?,
        revoked_count: row.try_get("revoked_count")?,
    })
}

fn validate_role(role: &str) -> Result<()> {
    if !matches!(role, "admin" | "member") {
        return Err(AccountError::InvalidInput("role must be admin or member").into());
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<()> {
    if !(12..=4096).contains(&password.len()) {
        return Err(AccountError::InvalidInput("password must contain 12 to 4096 bytes").into());
    }
    Ok(())
}

fn validate_credentials(username: &str, password: &str) -> Result<()> {
    if username.is_empty() || username.trim() != username || username.len() > 128 {
        return Err(AccountError::InvalidInput(
            "username must contain 1 to 128 bytes without surrounding whitespace",
        )
        .into());
    }
    validate_password(password)
}

async fn hash_password(password: &str) -> Result<String> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| anyhow::anyhow!("password hashing failed"))
    })
    .await?
}

async fn verify_password(password: &str, hash: &str) -> Result<bool> {
    if password.len() > 4096 {
        return Ok(false);
    }
    let password = password.to_owned();
    let hash = hash.to_owned();
    tokio::task::spawn_blocking(move || {
        let hash = PasswordHash::new(&hash)
            .map_err(|_| anyhow::anyhow!("invalid stored password hash"))?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok())
    })
    .await?
}

fn account_write_error(error: sqlx::Error) -> anyhow::Error {
    if error
        .as_database_error()
        .is_some_and(|error| error.is_unique_violation())
    {
        AccountError::Conflict("username already exists").into()
    } else {
        error.into()
    }
}

fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_user(row: &sqlx::any::AnyRow) -> Result<User> {
    Ok(User {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        display_name: row.try_get("display_name")?,
        role: row.try_get("role")?,
        source: row.try_get("source")?,
        disabled: row.try_get::<i64, _>("disabled")? != 0,
    })
}

fn token_digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn managed_account_contract() -> Result<()> {
        isolated_contract(false).await
    }

    #[tokio::test]
    async fn promotion_closes_bootstrap() -> Result<()> {
        let store = Store::connect("sqlite::memory:").await?;
        let member = store
            .create_local("member", "member password", "member")
            .await?;
        let oidc = store
            .find_or_create_oidc("https://promote.example", "admin", "Admin", Some("admin"))
            .await?
            .unwrap();
        let actor = store.issue_web_session(&oidc.id, 3600).await?;
        store
            .admin_update_user(&actor, &member.id, None, Some("admin"))
            .await?;
        ensure!(error_is(
            &store
                .bootstrap_admin("unexpected", "valid password")
                .await
                .unwrap_err(),
            "conflict"
        ));
        ensure!(error_is(
            &store.disable_user(&member.id).await.unwrap_err(),
            "conflict"
        ));
        store.pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn version_one_migration_contract() -> Result<()> {
        isolated_contract(true).await
    }

    async fn isolated_contract(migration: bool) -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sqlite = format!("sqlite://{}", directory.path().join("m1.sqlite").display());
        if migration {
            migration_contract(&sqlite).await?;
        } else {
            managed_contract(&sqlite).await?;
        }
        if let Ok(database_url) = std::env::var("DATABASE_URL") {
            ensure!(
                database_url.starts_with("postgres:") || database_url.starts_with("postgresql:"),
                "DATABASE_URL must be PostgreSQL"
            );
            sqlx::any::install_default_drivers();
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&database_url)
                .await?;
            let schema = format!("m1_{}", Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&pool)
                .await?;
            let mut scoped_url = url::Url::parse(&database_url)?;
            scoped_url
                .query_pairs_mut()
                .append_pair("options", &format!("-csearch_path={schema}"));
            let result = if migration {
                migration_contract(scoped_url.as_str()).await
            } else {
                managed_contract(scoped_url.as_str()).await
            };
            // Only this test's unique schema is removed; existing test data survives.
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&pool)
                .await?;
            pool.close().await;
            result?;
        }
        Ok(())
    }

    fn error_is(error: &anyhow::Error, expected: &str) -> bool {
        matches!(
            (error.downcast_ref::<AccountError>(), expected),
            (Some(AccountError::Unauthorized), "unauthorized")
                | (Some(AccountError::Forbidden), "forbidden")
                | (Some(AccountError::NotFound), "not_found")
                | (Some(AccountError::Conflict(_)), "conflict")
                | (Some(AccountError::InvalidInput(_)), "invalid")
        )
    }

    async fn managed_contract(url: &str) -> Result<()> {
        let (store, other) = tokio::try_join!(Store::connect(url), Store::connect(url))?;
        for (name, password) in [
            (" invalid", "valid password"),
            ("invalid ", "valid password"),
            ("", "valid password"),
            ("valid", "short"),
        ] {
            ensure!(error_is(
                &store
                    .create_local(name, password, "admin")
                    .await
                    .unwrap_err(),
                "invalid"
            ));
        }
        ensure!(
            store
                .create_local(&"a".repeat(129), "valid password", "admin")
                .await
                .is_err()
        );
        ensure!(
            store
                .create_local("valid", &"a".repeat(4097), "admin")
                .await
                .is_err()
        );
        let (first, second) = tokio::join!(
            store.bootstrap_admin("bootstrap-first", "initial password"),
            other.bootstrap_admin("bootstrap-second", "initial password")
        );
        ensure!(
            first.is_ok() != second.is_ok(),
            "bootstrap must have exactly one winner"
        );
        let admin = match (first, second) {
            (Ok(admin), Err(error)) | (Err(error), Ok(admin)) => {
                ensure!(error_is(&error, "conflict"));
                admin
            }
            _ => unreachable!(),
        };
        let (verified, admin_token) = store
            .login_local(&admin.username, "initial password", 3600, SessionKind::Web)
            .await?
            .unwrap();
        ensure!(verified.id == admin.id);
        let client_token = store
            .login_local(
                &admin.username,
                "initial password",
                3600,
                SessionKind::Client,
            )
            .await?
            .unwrap()
            .1;
        ensure!(store.authenticate(&admin_token).await?.is_none());
        ensure!(store.authenticate_web(&client_token).await?.is_none());
        ensure!(store.authenticate_web(&admin_token).await?.is_some());
        ensure!(
            store
                .login_local(&admin.username, "wrong password", 3600, SessionKind::Web)
                .await?
                .is_none()
        );
        ensure!(error_is(
            &store.disable_user(&admin.id).await.unwrap_err(),
            "conflict"
        ));
        ensure!(error_is(
            &store
                .admin_update_user(&admin_token, &admin.id, None, Some("member"))
                .await
                .unwrap_err(),
            "conflict"
        ));
        ensure!(error_is(
            &store
                .admin_update_user(&admin_token, &admin.id, Some(true), None)
                .await
                .unwrap_err(),
            "conflict"
        ));
        let member = store
            .admin_create_local(&admin_token, "member", "member password", "member")
            .await?;
        let member_token = store.issue_web_session(&member.id, 3600).await?;
        let member_client = store.issue_session(&member.id, 3600).await?;
        ensure!(error_is(
            &store.list_users(&member_token).await.err().unwrap(),
            "forbidden"
        ));
        ensure!(error_is(
            &store
                .admin_create_local(&client_token, "bad-client", "valid password", "member")
                .await
                .unwrap_err(),
            "unauthorized"
        ));
        ensure!(error_is(
            &store
                .admin_update_user(&admin_token, "missing", None, None)
                .await
                .unwrap_err(),
            "not_found"
        ));
        ensure!(error_is(
            &store
                .list_sessions(&member_token, Some(&admin.id))
                .await
                .err()
                .unwrap(),
            "forbidden"
        ));
        ensure!(error_is(
            &store
                .revoke_user_sessions(&member_token, Some(&admin.id))
                .await
                .unwrap_err(),
            "forbidden"
        ));
        let admin_sessions = store.list_sessions(&admin_token, None).await?;
        ensure!(
            admin_sessions.len() == 2 && admin_sessions.iter().filter(|s| s.current).count() == 1
        );
        for session in &admin_sessions {
            Uuid::parse_str(&session.id)?;
            ensure!(session.created_at > 0 && session.expires_at > session.created_at);
        }
        ensure!(error_is(
            &store
                .revoke_managed_session(&member_token, &admin_sessions[0].id)
                .await
                .unwrap_err(),
            "forbidden"
        ));
        let member_sessions = store.list_sessions(&admin_token, Some(&member.id)).await?;
        let client_id = &member_sessions
            .iter()
            .find(|s| s.kind == "client")
            .unwrap()
            .id;
        store
            .revoke_managed_session(&member_token, client_id)
            .await?;
        ensure!(store.authenticate(&member_client).await?.is_none());
        let user_json = serde_json::to_value(store.list_users(&admin_token).await?)?;
        let session_json = serde_json::to_value(store.list_sessions(&member_token, None).await?)?;
        ensure!(
            !user_json.to_string().contains("password_hash")
                && !session_json.to_string().contains("digest")
        );

        // Simulate a reset between the expensive password verification and issuance.
        let (_, verified_hash) = store.local_credentials("member").await?.unwrap();
        store
            .admin_reset_password(&admin_token, &member.id, "replacement password")
            .await?;
        ensure!(store.authenticate_web(&member_token).await?.is_none());
        ensure!(
            store
                .finish_local_login(&member.id, &verified_hash, 3600, SessionKind::Web)
                .await?
                .is_none()
        );
        ensure!(
            store
                .login_local("member", "member password", 3600, SessionKind::Web)
                .await?
                .is_none()
        );
        let changed_token = store
            .login_local("member", "replacement password", 3600, SessionKind::Web)
            .await?
            .unwrap()
            .1;
        let changed_client = store.issue_session(&member.id, 3600).await?;
        ensure!(error_is(
            &store
                .change_password(&changed_token, "incorrect password", "third new password")
                .await
                .unwrap_err(),
            "unauthorized"
        ));
        store
            .change_password(&changed_token, "replacement password", "third new password")
            .await?;
        ensure!(
            store.authenticate_web(&changed_token).await?.is_none()
                && store.authenticate(&changed_client).await?.is_none()
        );
        ensure!(
            store
                .login_local("member", "replacement password", 3600, SessionKind::Web)
                .await?
                .is_none()
        );
        let member_token = store
            .login_local("member", "third new password", 3600, SessionKind::Web)
            .await?
            .unwrap()
            .1;
        store.revoke_user_sessions(&member_token, None).await?;
        ensure!(store.authenticate_web(&member_token).await?.is_none());

        let oidc = store
            .find_or_create_oidc("https://m1.example", "subject", "OIDC", Some("admin"))
            .await?
            .unwrap();
        let oidc_web = store.issue_web_session(&oidc.id, 3600).await?;
        ensure!(error_is(
            &store
                .admin_update_user(&admin_token, &oidc.id, None, Some("member"))
                .await
                .unwrap_err(),
            "conflict"
        ));
        ensure!(error_is(
            &store
                .admin_reset_password(&admin_token, &oidc.id, "valid password")
                .await
                .unwrap_err(),
            "conflict"
        ));
        ensure!(error_is(
            &store
                .change_password(&oidc_web, "any password", "valid password")
                .await
                .unwrap_err(),
            "conflict"
        ));
        // OIDC admin downgrades must not invoke the final-local-admin guard.
        store
            .find_or_create_oidc("https://m1.example", "subject", "OIDC", Some("member"))
            .await?;
        ensure!(store.authenticate_web(&oidc_web).await?.is_none());
        let oidc_web = store.issue_web_session(&oidc.id, 3600).await?;
        store
            .admin_update_user(&admin_token, &oidc.id, Some(true), None)
            .await?;
        store
            .admin_update_user(&admin_token, &oidc.id, Some(false), None)
            .await?;
        ensure!(store.issue_web_session(&oidc.id, 3600).await.is_err());
        let listed = store.list_users(&admin_token).await?;
        ensure!(
            listed
                .iter()
                .any(|u| u.id == oidc.id && !u.disabled && !u.admitted)
        );
        store
            .find_or_create_oidc("https://m1.example", "subject", "OIDC", Some("member"))
            .await?;
        ensure!(store.authenticate_web(&oidc_web).await?.is_none());

        let spare = store
            .admin_create_local(&admin_token, "spare", "spare admin password", "admin")
            .await?;
        let spare_web = store.issue_web_session(&spare.id, 3600).await?;
        // Each writer rechecks both the actor and the final effective local admin.
        let (demote, disable) = tokio::join!(
            store.admin_update_user(&admin_token, &admin.id, None, Some("member")),
            other.admin_update_user(&spare_web, &spare.id, Some(true), None)
        );
        ensure!(
            demote.is_ok() != disable.is_ok(),
            "concurrent changes removed all local admins"
        );
        let live_admin = if demote.is_ok() {
            &spare_web
        } else {
            &admin_token
        };
        let stale_admin = if demote.is_ok() {
            &admin_token
        } else {
            &spare_web
        };
        ensure!(store.authenticate_web(stale_admin).await?.is_none());
        ensure!(error_is(
            &store
                .admin_reset_password(stale_admin, &member.id, "valid password")
                .await
                .unwrap_err(),
            "unauthorized"
        ));
        ensure!(
            store
                .list_users(live_admin)
                .await?
                .iter()
                .filter(|u| u.source == "local" && u.role == "admin" && !u.disabled && u.admitted)
                .count()
                == 1
        );
        store.pool.close().await;
        other.pool.close().await;
        let reopened = Store::connect(url).await?;
        ensure!(error_is(
            &reopened
                .bootstrap_admin("again", "valid password")
                .await
                .unwrap_err(),
            "conflict"
        ));
        reopened.pool.close().await;
        Ok(())
    }

    async fn migration_contract(url: &str) -> Result<()> {
        sqlx::any::install_default_drivers();
        let sqlite = url.starts_with("sqlite:");
        let connection_url = if sqlite {
            format!("{url}?mode=rwc")
        } else {
            url.to_owned()
        };
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect(&connection_url)
            .await?;
        sqlx::raw_sql(if sqlite {
            SQLITE_INITIAL
        } else {
            POSTGRES_INITIAL
        })
        .execute(&pool)
        .await?;
        sqlx::raw_sql("CREATE TABLE account_schema_migrations (version BIGINT PRIMARY KEY NOT NULL); INSERT INTO account_schema_migrations (version) VALUES (1);").execute(&pool).await?;
        // Use the dialect binder without invoking migrations.
        let old = Store {
            pool,
            postgres: !sqlite,
            credential_cipher: None,
        };
        let hash = hash_password("legacy password").await?;
        sqlx::query(&old.sql("INSERT INTO users (id, username, display_name, role, source, password_hash) VALUES ('legacy', 'legacy', 'Legacy', 'admin', 'local', ?)"))
            .bind(hash).execute(&old.pool).await?;
        sqlx::query(r#"INSERT INTO books (user_id, snapshot) VALUES ('legacy', '{"peers":[{"id":"kept"}],"tags":[]}')"#).execute(&old.pool).await?;
        for token in ["a".repeat(64), "b".repeat(64)] {
            sqlx::query(
                &old.sql(
                    "INSERT INTO sessions (digest, user_id, expires_at) VALUES (?, 'legacy', ?)",
                ),
            )
            .bind(token_digest(&token))
            .bind(chrono::Utc::now().timestamp() + 3600)
            .execute(&old.pool)
            .await?;
        }
        old.pool.close().await;
        let (store, other) = tokio::try_join!(Store::connect(url), Store::connect(url))?;
        ensure!(store.authenticate(&"a".repeat(64)).await?.is_some());
        ensure!(store.authenticate_web(&"a".repeat(64)).await?.is_none());
        ensure!(
            store
                .verify_local("legacy", "legacy password")
                .await?
                .is_some()
        );
        ensure!(store.get_book("legacy").await? == json!({"peers":[{"id":"kept"}], "tags":[]}));
        ensure!(error_is(
            &store
                .bootstrap_admin("blocked", "valid password")
                .await
                .unwrap_err(),
            "conflict"
        ));
        let web = store.issue_web_session("legacy", 3600).await?;
        let sessions = store.list_sessions(&web, None).await?;
        ensure!(sessions.len() == 3);
        let mut ids = std::collections::HashSet::new();
        for session in sessions {
            Uuid::parse_str(&session.id)?;
            ensure!(ids.insert(session.id));
        }
        let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM account_schema_migrations")
            .fetch_one(&store.pool)
            .await?;
        ensure!(version == 4);
        ensure!(error_is(
            &store.disable_user("legacy").await.unwrap_err(),
            "conflict"
        ));
        store.pool.close().await;
        other.pool.close().await;
        Ok(())
    }

    /// The same behavior contract runs against a file-backed SQLite database and,
    /// when supplied, a dedicated PostgreSQL test database. No database is cleared.
    #[tokio::test]
    async fn persistent_account_contract() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sqlite = format!(
            "sqlite://{}",
            directory.path().join("accounts.sqlite").display()
        );
        account_contract(&sqlite).await?;
        if let Ok(url) = std::env::var("DATABASE_URL") {
            ensure!(
                url.starts_with("postgres:") || url.starts_with("postgresql:"),
                "DATABASE_URL for the optional store test must be PostgreSQL"
            );
            account_contract(&url).await?;
        }
        Ok(())
    }

    async fn account_contract(url: &str) -> Result<()> {
        let namespace = Uuid::new_v4().to_string();
        let username = format!("local-{namespace}");
        let issuer = format!("https://identity.example/{namespace}");
        let (store, startup_peer) = tokio::try_join!(Store::connect(url), Store::connect(url))?;
        startup_peer.pool.close().await;
        let local = store
            .create_local(&username, "correct horse battery staple", "admin")
            .await?;
        ensure!(
            store
                .verify_local(&username, "wrong password")
                .await?
                .is_none()
        );
        ensure!(
            store
                .verify_local(&username, "correct horse battery staple")
                .await?
                .map(|u| u.id)
                == Some(local.id.clone())
        );
        ensure!(
            store
                .create_local(&username, "other password", "member")
                .await
                .is_err()
        );
        let stored_hash: String =
            sqlx::query_scalar(&store.sql("SELECT password_hash FROM users WHERE id = ?"))
                .bind(&local.id)
                .fetch_one(&store.pool)
                .await?;
        ensure!(stored_hash.starts_with("$argon2id$") && !stored_hash.contains("correct horse"));

        let token = store.issue_session(&local.id, 3600).await?;
        let second_token = store.issue_session(&local.id, 3600).await?;
        ensure!(token != second_token && token.len() == 64);
        let stored_digest: String =
            sqlx::query_scalar(&store.sql("SELECT digest FROM sessions WHERE digest = ?"))
                .bind(token_digest(&token))
                .fetch_one(&store.pool)
                .await?;
        ensure!(stored_digest != token);
        let book = json!({"peers": [{"id": "peer-1", "alias": "中文设备"}], "tags": ["work"], "revision": 9});
        store.put_book(&local.id, &book).await?;
        ensure!(store.put_book("missing-owner", &book).await.is_err());
        store.pool.close().await;

        // Data and credentials survive destruction/recreation of the connection pool.
        let store = Store::connect(url).await?;
        ensure!(store.authenticate(&token).await?.map(|u| u.id) == Some(local.id.clone()));
        ensure!(store.get_book(&local.id).await? == book);
        ensure!(
            store
                .verify_local(&username, "correct horse battery staple")
                .await?
                .is_some()
        );
        store.revoke_session(&token).await?;
        ensure!(store.authenticate(&token).await?.is_none());
        ensure!(store.authenticate(&second_token).await?.is_some());
        ensure!(store.issue_session(&local.id, 0).await.is_err());
        let expired = store.issue_session(&local.id, 3600).await?;
        sqlx::query(&store.sql("UPDATE sessions SET expires_at = ? WHERE digest = ?"))
            .bind(chrono::Utc::now().timestamp() - 1)
            .bind(token_digest(&expired))
            .execute(&store.pool)
            .await?;
        ensure!(store.authenticate(&expired).await?.is_none());

        // Independent pools model separate service processes racing the same issuer/sub.
        let other = Store::connect(url).await?;
        let mut tasks = Vec::new();
        for index in 0..12 {
            let writer = if index % 2 == 0 {
                store.clone()
            } else {
                other.clone()
            };
            let issuer = issuer.clone();
            tasks.push(tokio::spawn(async move {
                writer
                    .find_or_create_oidc(&issuer, "stable-subject", "Initial name", Some("admin"))
                    .await
            }));
        }
        let mut ids = std::collections::HashSet::new();
        for task in tasks {
            ids.insert(task.await??.context("admitted OIDC user missing")?.id);
        }
        ensure!(
            ids.len() == 1,
            "concurrent OIDC login created duplicate identities"
        );
        let oidc_id = ids.into_iter().next().unwrap();
        ensure!(store.get_book(&oidc_id).await? == json!({"peers": [], "tags": []}));
        let oidc_book = json!({"peers": [{"id": "private-peer"}], "tags": []});
        other.put_book(&oidc_id, &oidc_book).await?;
        ensure!(store.get_book(&local.id).await? == book);
        ensure!(store.get_book(&oidc_id).await? == oidc_book);
        let other_identity = store
            .find_or_create_oidc(&issuer, "different-subject", "Initial name", Some("member"))
            .await?
            .unwrap();
        let other_issuer = store
            .find_or_create_oidc(
                &format!("{issuer}/other"),
                "stable-subject",
                "Initial name",
                Some("member"),
            )
            .await?
            .unwrap();
        ensure!(other_identity.id != oidc_id && other_issuer.id != oidc_id);

        let admin_session = store.issue_session(&oidc_id, 3600).await?;
        let downgraded = other
            .find_or_create_oidc(&issuer, "stable-subject", "Renamed", Some("member"))
            .await?
            .unwrap();
        ensure!(
            downgraded.id == oidc_id
                && downgraded.role == "member"
                && downgraded.display_name == "Renamed"
        );
        ensure!(store.authenticate(&admin_session).await?.is_none());
        let member_session = store.issue_session(&oidc_id, 3600).await?;
        ensure!(
            other
                .find_or_create_oidc(&issuer, "stable-subject", "Denied", None)
                .await?
                .is_none()
        );
        ensure!(store.authenticate(&member_session).await?.is_none());
        ensure!(store.issue_session(&oidc_id, 3600).await.is_err());
        other.pool.close().await;
        let other = Store::connect(url).await?;
        ensure!(
            other.issue_session(&oidc_id, 3600).await.is_err(),
            "denied admission did not persist"
        );
        let denied_role: String =
            sqlx::query_scalar(&other.sql("SELECT role FROM users WHERE id = ?"))
                .bind(&oidc_id)
                .fetch_one(&other.pool)
                .await?;
        ensure!(denied_role.is_empty());
        ensure!(
            store
                .find_or_create_oidc(&issuer, "never-admitted", "Denied new user", None)
                .await?
                .is_none()
        );
        let denied_id: String =
            sqlx::query_scalar(&store.sql("SELECT id FROM users WHERE issuer = ? AND subject = ?"))
                .bind(&issuer)
                .bind("never-admitted")
                .fetch_one(&store.pool)
                .await?;
        ensure!(store.issue_session(&denied_id, 3600).await.is_err());

        store
            .find_or_create_oidc(&issuer, "stable-subject", "Readmitted", Some("member"))
            .await?
            .unwrap();
        ensure!(
            store.authenticate(&member_session).await?.is_none(),
            "readmission revived an old token"
        );
        let policy_v1 = format!("{namespace}-v1");
        let policy_v2 = format!("{namespace}-v2");
        store.sync_policy_version(&policy_v1).await?;
        let policy_session = store.issue_session(&oidc_id, 3600).await?;
        other.sync_policy_version(&policy_v1).await?;
        ensure!(store.authenticate(&policy_session).await?.is_some());
        other.sync_policy_version(&policy_v2).await?;
        ensure!(store.authenticate(&policy_session).await?.is_none());
        ensure!(
            store.authenticate(&second_token).await?.is_some(),
            "OIDC policy change revoked a local session"
        );
        let disabled_session = store.issue_session(&oidc_id, 3600).await?;
        store.disable_user(&oidc_id).await?;
        ensure!(
            other
                .find_or_create_oidc(&issuer, "stable-subject", "Still disabled", Some("admin"))
                .await?
                .is_none()
        );
        ensure!(store.authenticate(&disabled_session).await?.is_none());
        ensure!(store.issue_session(&oidc_id, 3600).await.is_err());
        store
            .create_local(
                &format!("spare-{namespace}"),
                "spare admin password",
                "admin",
            )
            .await?;
        store.disable_user(&local.id).await?;
        ensure!(
            store
                .verify_local(&username, "correct horse battery staple")
                .await?
                .is_none()
        );
        ensure!(store.authenticate(&second_token).await?.is_none());
        store.pool.close().await;
        other.pool.close().await;
        Ok(())
    }
}
