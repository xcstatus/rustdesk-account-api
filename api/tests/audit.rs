use anyhow::Result;
use rustdesk_account_api::store::{
    AccountError, AuditAction, AuditFilter, AuditKind, AuditReason, AuditResult, AuditRole,
    AuditSource, SessionKind, Store,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};

const PASSWORD: &str = "secret-password-never-in-audit";

async fn database() -> Result<(tempfile::TempDir, Store, AnyPool, String)> {
    let directory = tempfile::tempdir()?;
    let url = format!(
        "sqlite://{}",
        directory.path().join("audit.sqlite").display()
    );
    let store = Store::connect(&url).await?;
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    Ok((directory, store, pool, url))
}

#[tokio::test]
async fn typed_audit_permissions_pagination_filters_and_privacy() -> Result<()> {
    let (_directory, store, pool, _) = database().await?;
    let admin = store
        .bootstrap_admin("secret-administrator-name", PASSWORD)
        .await?;
    let admin_web = store.issue_web_session(&admin.id, 3600).await?;
    let admin_client = store.issue_session(&admin.id, 3600).await?;
    let member = store
        .admin_create_local(&admin_web, "secret-member-name", PASSWORD, "member")
        .await?;
    let member_web = store.issue_web_session(&member.id, 3600).await?;
    assert!(matches!(
        store
            .list_audit_events(&member_web, &AuditFilter::default())
            .await
            .unwrap_err()
            .downcast_ref::<AccountError>(),
        Some(AccountError::Forbidden)
    ));
    assert!(matches!(
        store
            .list_audit_events(&admin_client, &AuditFilter::default())
            .await
            .unwrap_err()
            .downcast_ref::<AccountError>(),
        Some(AccountError::Unauthorized)
    ));
    assert!(
        store
            .list_audit_events("invalid", &AuditFilter::default())
            .await
            .is_err()
    );
    assert!(
        store
            .login_local("secret-nonexistent-name", PASSWORD, 3600, SessionKind::Web)
            .await?
            .is_none()
    );
    assert!(
        store
            .login_local(
                "secret-member-name",
                "wrong-secret-password",
                3600,
                SessionKind::Client
            )
            .await?
            .is_none()
    );
    store
        .record_login_failure("oidc", None, "oidc_verification")
        .await?;
    assert!(
        store
            .record_login_failure("raw-secret", None, "invalid_request")
            .await
            .is_err()
    );
    assert!(
        store
            .record_login_failure("local", None, "raw-secret")
            .await
            .is_err()
    );
    let oidc = store
        .find_or_create_oidc(
            "https://secret-issuer.example",
            "secret-subject",
            "secret-display-name",
            Some("member"),
        )
        .await?
        .unwrap();
    assert!(
        store
            .find_or_create_oidc(
                "https://secret-issuer.example",
                "secret-denied-subject",
                "secret-denied-name",
                None
            )
            .await?
            .is_none()
    );
    store
        .admin_update_user(&admin_web, &member.id, Some(true), Some("admin"))
        .await?;
    let now = chrono::Utc::now().timestamp();
    let filter = AuditFilter {
        actor_id: Some(admin.id.clone()),
        target_id: Some(member.id.clone()),
        action: Some(AuditAction::UpdateUser),
        source: Some(AuditSource::Local),
        kind: Some(AuditKind::Web),
        result: Some(AuditResult::Success),
        since: Some(now - 30),
        until: Some(now + 30),
        ..AuditFilter::default()
    };
    let filtered = store.list_audit_events(&admin_web, &filter).await?;
    assert_eq!(filtered.events.len(), 1);
    let update = &filtered.events[0];
    assert_eq!(update.before_role, Some(AuditRole::Member));
    assert_eq!(update.after_role, Some(AuditRole::Admin));
    assert_eq!(update.before_disabled, Some(false));
    assert_eq!(update.after_disabled, Some(true));
    assert_eq!(update.before_admitted, Some(true));
    assert_eq!(update.after_admitted, Some(false));
    assert_eq!(update.revoked_count, 1);
    assert!(
        store
            .list_audit_events(
                &admin_web,
                &AuditFilter {
                    actor_id: Some("' OR 1=1 --".into()),
                    ..AuditFilter::default()
                }
            )
            .await?
            .events
            .is_empty()
    );
    for limit in [0, 201] {
        assert!(
            store
                .list_audit_events(
                    &admin_web,
                    &AuditFilter {
                        limit,
                        ..AuditFilter::default()
                    }
                )
                .await
                .is_err()
        );
    }
    assert!(serde_json::from_value::<AuditFilter>(json!({"metadata":"forbidden"})).is_err());
    assert!(serde_json::from_value::<AuditFilter>(json!({"action":"untrusted"})).is_err());
    assert_eq!(serde_json::from_value::<AuditFilter>(json!({}))?.limit, 50);
    let all = store
        .list_audit_events(&admin_web, &AuditFilter::default())
        .await?;
    let serialized = serde_json::to_string(&all)?;
    let digest = hex::encode(Sha256::digest(admin_web.as_bytes()));
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(&admin.id)
        .fetch_one(&pool)
        .await?;
    for forbidden in [
        PASSWORD,
        "secret-administrator-name",
        "secret-member-name",
        "secret-subject",
        "secret-display-name",
        "secret-issuer",
        "secret-denied-subject",
        &admin_web,
        &admin_client,
        &member_web,
        &digest,
        &hash,
    ] {
        assert!(
            !serialized.contains(forbidden),
            "audit contained private data"
        );
    }
    assert!(
        all.events
            .iter()
            .any(|e| e.reason == Some(AuditReason::InvalidCredentials))
    );
    assert!(
        all.events
            .iter()
            .any(|e| e.target_id.as_deref() == Some(&oidc.id))
    );
    let first = store
        .list_audit_events(
            &admin_web,
            &AuditFilter {
                limit: 2,
                ..AuditFilter::default()
            },
        )
        .await?;
    store
        .record_login_failure("local", Some(SessionKind::Web), "rate_limited")
        .await?;
    let mut ids: Vec<i64> = first.events.iter().map(|e| e.id).collect();
    let mut cursor = first.next_before_id;
    while let Some(before_id) = cursor {
        let page = store
            .list_audit_events(
                &admin_web,
                &AuditFilter {
                    before_id: Some(before_id),
                    limit: 2,
                    ..AuditFilter::default()
                },
            )
            .await?;
        ids.extend(page.events.iter().map(|e| e.id));
        cursor = page.next_before_id;
    }
    assert_eq!(ids, all.events.iter().map(|e| e.id).collect::<Vec<_>>());
    assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
    store.revoke_session(&admin_web).await?;
    assert!(
        store
            .list_audit_events(&admin_web, &AuditFilter::default())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn audit_insert_failure_rolls_back_account_session_and_policy_changes() -> Result<()> {
    let (_directory, store, pool, _) = database().await?;
    let admin = store.bootstrap_admin("admin", PASSWORD).await?;
    let admin_web = store.issue_web_session(&admin.id, 3600).await?;
    let member = store
        .admin_create_local(&admin_web, "member", PASSWORD, "member")
        .await?;
    let member_web = store.issue_web_session(&member.id, 3600).await?;
    let member_client = store.issue_session(&member.id, 3600).await?;
    let oidc = store
        .find_or_create_oidc("https://issuer.example", "subject", "name", Some("member"))
        .await?
        .unwrap();
    let oidc_token = store.issue_session(&oidc.id, 3600).await?;
    let member_session = store
        .list_sessions(&admin_web, Some(&member.id))
        .await?
        .into_iter()
        .find(|s| s.kind == "client")
        .unwrap();
    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await?;
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(&pool)
        .await?;
    sqlx::raw_sql("CREATE TRIGGER reject_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;").execute(&pool).await?;
    assert!(
        store
            .create_local("rolled-back-cli", PASSWORD, "admin")
            .await
            .is_err()
    );
    assert!(
        store
            .admin_create_local(&admin_web, "rolled-back-admin", PASSWORD, "member")
            .await
            .is_err()
    );
    assert!(
        store
            .admin_update_user(&admin_web, &member.id, Some(true), Some("admin"))
            .await
            .is_err()
    );
    assert!(
        store
            .admin_reset_password(&admin_web, &member.id, "new-secret-password")
            .await
            .is_err()
    );
    assert!(
        store
            .change_password(&member_web, PASSWORD, "new-secret-password")
            .await
            .is_err()
    );
    assert!(
        store
            .find_or_create_oidc("https://issuer.example", "new-denied", "name", None)
            .await
            .is_err()
    );
    assert!(
        store
            .find_or_create_oidc("https://issuer.example", "subject", "renamed", None)
            .await
            .is_err()
    );
    assert!(store.sync_policy_version("new-policy").await.is_err());
    assert!(store.issue_session(&member.id, 3600).await.is_err());
    assert!(
        store
            .login_local("member", PASSWORD, 3600, SessionKind::Web)
            .await
            .is_err()
    );
    assert!(
        store
            .login_local("missing", PASSWORD, 3600, SessionKind::Web)
            .await
            .is_err()
    );
    assert!(
        store
            .logout_session(&member_client, SessionKind::Client)
            .await
            .is_err()
    );
    assert!(store.revoke_session(&member_client).await.is_err());
    assert!(
        store
            .revoke_managed_session(&admin_web, &member_session.id)
            .await
            .is_err()
    );
    assert!(
        store
            .revoke_user_sessions(&admin_web, Some(&member.id))
            .await
            .is_err()
    );
    assert!(store.disable_user(&member.id).await.is_err());
    assert!(store.readiness().await.is_err());
    sqlx::query("UPDATE audit_events SET timestamp = 1")
        .execute(&pool)
        .await?;
    assert!(store.prune_audit(2, 1).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_events")
            .fetch_one(&pool)
            .await?,
        event_count
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await?,
        session_count
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await?,
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='policy_version'")
            .fetch_one(&pool)
            .await?,
        ""
    );
    let unchanged = store.verify_local("member", PASSWORD).await?.unwrap();
    assert_eq!(unchanged.role, "member");
    assert!(!unchanged.disabled);
    assert!(store.authenticate(&member_client).await?.is_some());
    assert!(store.authenticate_web(&member_web).await?.is_some());
    assert!(store.authenticate(&oidc_token).await?.is_some());
    assert_eq!(
        store.authenticate(&oidc_token).await?.unwrap().display_name,
        "name"
    );
    sqlx::query("DROP TRIGGER reject_audit")
        .execute(&pool)
        .await?;
    store.readiness().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_events")
            .fetch_one(&pool)
            .await?,
        event_count
    );
    Ok(())
}

#[tokio::test]
async fn bootstrap_and_its_latch_roll_back_when_audit_fails() -> Result<()> {
    let (_directory, store, pool, _) = database().await?;
    sqlx::raw_sql("CREATE TRIGGER reject_audit BEFORE INSERT ON audit_events BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;").execute(&pool).await?;
    assert!(store.bootstrap_admin("first", PASSWORD).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM settings WHERE key='bootstrap_initialized'"
        )
        .fetch_one(&pool)
        .await?,
        "0"
    );
    sqlx::query("DROP TRIGGER reject_audit")
        .execute(&pool)
        .await?;
    store.bootstrap_admin("second", PASSWORD).await?;
    Ok(())
}

#[tokio::test]
async fn retention_is_bounded_strict_and_does_not_create_empty_batch_noise() -> Result<()> {
    let (_directory, store, pool, _) = database().await?;
    let admin = store.bootstrap_admin("admin", PASSWORD).await?;
    let token = store.issue_web_session(&admin.id, 3600).await?;
    store
        .record_login_failure("local", None, "invalid_request")
        .await?;
    sqlx::query("UPDATE audit_events SET timestamp = id * 10")
        .execute(&pool)
        .await?;
    assert_eq!(store.prune_audit(20, 1).await?, 1);
    let after = store
        .list_audit_events(&token, &AuditFilter::default())
        .await?;
    assert_eq!(after.events.len(), 3);
    assert!(after.events.iter().any(|e| e.timestamp == 20));
    assert!(!after.events.iter().any(|e| e.timestamp < 20));
    assert_eq!(after.events[0].action, AuditAction::PruneAudit);
    assert_eq!(after.events[0].revoked_count, 0);
    assert_eq!(store.prune_audit(20, 1).await?, 0);
    assert_eq!(
        store
            .list_audit_events(&token, &AuditFilter::default())
            .await?
            .events
            .len(),
        3
    );
    let boundary = store
        .list_audit_events(
            &token,
            &AuditFilter {
                since: Some(20),
                until: Some(20),
                ..AuditFilter::default()
            },
        )
        .await?;
    assert_eq!(boundary.events.len(), 1);
    assert!(
        store
            .prune_audit(chrono::Utc::now().timestamp() + 60, 1)
            .await
            .is_err()
    );
    assert!(store.prune_audit(20, 0).await.is_err());
    assert!(store.prune_audit(20, 10_001).await.is_err());
    Ok(())
}

#[tokio::test]
async fn version_two_upgrade_preserves_accounts_sessions_books_and_rejects_future_schema()
-> Result<()> {
    sqlx::any::install_default_drivers();
    let directory = tempfile::tempdir()?;
    let url = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("legacy.sqlite").display()
    );
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/sqlite/0001_initial.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/sqlite/0002_accounts.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql("CREATE TABLE account_schema_migrations(version BIGINT PRIMARY KEY NOT NULL); INSERT INTO account_schema_migrations(version) VALUES (1),(2); CREATE UNIQUE INDEX sessions_public_id_idx ON sessions(id); INSERT INTO users(id,username,display_name,role,source,password_hash) VALUES ('legacy','legacy','Legacy','member','local','unused'); INSERT INTO books(user_id,snapshot) VALUES ('legacy','{\"peers\":[{\"id\":\"kept\"}],\"tags\":[]}');").execute(&pool).await?;
    let bearer = "a".repeat(64);
    sqlx::query("INSERT INTO sessions(digest,id,user_id,kind,created_at,expires_at) VALUES (?,'public-session','legacy','client',1,?)")
        .bind(hex::encode(Sha256::digest(bearer.as_bytes()))).bind(chrono::Utc::now().timestamp() + 3600).execute(&pool).await?;
    let (store, peer) = tokio::try_join!(Store::connect(&url), Store::connect(&url))?;
    store.readiness().await?;
    peer.readiness().await?;
    assert_eq!(store.authenticate(&bearer).await?.unwrap().id, "legacy");
    assert_eq!(
        store.get_book_authenticated(&bearer).await?,
        json!({"peers":[{"id":"kept"}],"tags":[]})
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM account_schema_migrations")
            .fetch_one(&pool)
            .await?,
        4
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_events")
            .fetch_one(&pool)
            .await?,
        0
    );
    let row = sqlx::query("SELECT id,created_at FROM sessions")
        .fetch_one(&pool)
        .await?;
    assert_eq!(row.try_get::<String, _>("id")?, "public-session");
    assert_eq!(row.try_get::<i64, _>("created_at")?, 1);
    sqlx::query("INSERT INTO account_schema_migrations(version) VALUES (5)")
        .execute(&pool)
        .await?;
    assert!(Store::connect(&url).await.is_err());
    assert!(store.readiness().await.is_err());
    Ok(())
}

#[tokio::test]
async fn logout_kind_is_enforced_and_internal_revoke_is_not_logout() -> Result<()> {
    let (_directory, store, _pool, _) = database().await?;
    let admin = store.bootstrap_admin("admin", PASSWORD).await?;
    let web = store.issue_web_session(&admin.id, 3600).await?;
    let client = store.issue_session(&admin.id, 3600).await?;
    assert!(
        store
            .logout_session(&web, SessionKind::Client)
            .await
            .is_err()
    );
    assert!(store.authenticate_web(&web).await?.is_some());
    store.logout_session(&client, SessionKind::Client).await?;
    assert!(store.authenticate(&client).await?.is_none());
    let internal = store.issue_session(&admin.id, 3600).await?;
    store.revoke_session(&internal).await?;
    let events = store
        .list_audit_events(&web, &AuditFilter::default())
        .await?
        .events;
    let logouts = events
        .iter()
        .filter(|event| event.action == AuditAction::Logout)
        .collect::<Vec<_>>();
    assert_eq!(logouts.len(), 1);
    assert_eq!(logouts[0].kind, AuditKind::Client);
    assert_eq!(logouts[0].revoked_count, 1);
    assert!(logouts[0].session_id.is_some());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.action == AuditAction::RevokeSession)
            .count(),
        1
    );
    assert!(store.issue_session("missing", 3600).await.is_err());
    assert!(store.issue_session(&admin.id, 0).await.is_err());
    let failures = store
        .list_audit_events(
            &web,
            &AuditFilter {
                action: Some(AuditAction::Login),
                result: Some(AuditResult::Failure),
                ..AuditFilter::default()
            },
        )
        .await?;
    assert!(
        failures
            .events
            .iter()
            .any(|e| e.reason == Some(AuditReason::SessionDenied))
    );
    assert!(
        failures
            .events
            .iter()
            .any(|e| e.reason == Some(AuditReason::InvalidRequest))
    );
    Ok(())
}

#[tokio::test]
async fn successful_lifecycle_records_closed_actions_and_current_policy() -> Result<()> {
    let (_directory, store, _pool, _) = database().await?;
    let admin = store.bootstrap_admin("admin", PASSWORD).await?;
    let web = store.issue_web_session(&admin.id, 3600).await?;
    let member = store
        .admin_create_local(&web, "member", PASSWORD, "member")
        .await?;
    let first = store.issue_session(&member.id, 3600).await?;
    let public_id = store.list_sessions(&web, Some(&member.id)).await?[0]
        .id
        .clone();
    store.revoke_managed_session(&web, &public_id).await?;
    assert!(store.authenticate(&first).await?.is_none());
    let _ = store.issue_session(&member.id, 3600).await?;
    store.revoke_user_sessions(&web, Some(&member.id)).await?;
    store
        .admin_reset_password(&web, &member.id, "reset-secret-password")
        .await?;
    let member_web = store
        .login_local("member", "reset-secret-password", 3600, SessionKind::Web)
        .await?
        .unwrap()
        .1;
    store
        .change_password(
            &member_web,
            "reset-secret-password",
            "changed-secret-password",
        )
        .await?;
    assert!(store.authenticate_web(&member_web).await?.is_none());
    let oidc = store
        .find_or_create_oidc(
            "https://issuer.example",
            "subject",
            "display",
            Some("admin"),
        )
        .await?
        .unwrap();
    let oidc_web = store.issue_web_session(&oidc.id, 3600).await?;
    let created_by_oidc = store
        .admin_create_local(&oidc_web, "oidc-created-local", PASSWORD, "member")
        .await?;
    store.logout_session(&oidc_web, SessionKind::Web).await?;
    let oidc_session = store.issue_session(&oidc.id, 3600).await?;
    store
        .find_or_create_oidc(
            "https://issuer.example",
            "subject",
            "display",
            Some("member"),
        )
        .await?;
    assert!(store.authenticate(&oidc_session).await?.is_none());
    let current_oidc = store.issue_session(&oidc.id, 3600).await?;
    store.sync_policy_version("policy-v2").await?;
    assert!(store.authenticate(&current_oidc).await?.is_none());
    store.sync_policy_version("policy-v2").await?;
    let events = store
        .list_audit_events(&web, &AuditFilter::default())
        .await?
        .events;
    for action in [
        AuditAction::Bootstrap,
        AuditAction::CreateUser,
        AuditAction::Login,
        AuditAction::RevokeSession,
        AuditAction::RevokeAll,
        AuditAction::ResetPassword,
        AuditAction::ChangePassword,
        AuditAction::OidcJit,
        AuditAction::OidcMapping,
        AuditAction::PolicyChange,
    ] {
        assert!(events.iter().any(|e| e.action == action));
    }
    assert_eq!(
        events
            .iter()
            .find(|e| e.action == AuditAction::Bootstrap)
            .unwrap()
            .source,
        AuditSource::System
    );
    let created = events
        .iter()
        .find(|e| {
            e.action == AuditAction::CreateUser
                && e.target_id.as_deref() == Some(&created_by_oidc.id)
        })
        .unwrap();
    assert_eq!(created.source, AuditSource::Oidc);
    assert_eq!(created.actor_id.as_deref(), Some(oidc.id.as_str()));
    let policy = events
        .iter()
        .filter(|e| e.action == AuditAction::PolicyChange)
        .collect::<Vec<_>>();
    assert_eq!(policy.len(), 1);
    assert_eq!(policy[0].policy_version, "policy-v2");
    assert_eq!(policy[0].revoked_count, 1);
    let mapping = events
        .iter()
        .find(|e| e.action == AuditAction::OidcMapping)
        .unwrap();
    assert_eq!(mapping.before_role, Some(AuditRole::Admin));
    assert_eq!(mapping.after_role, Some(AuditRole::Member));
    assert_eq!(mapping.revoked_count, 1);
    assert_eq!(
        events
            .iter()
            .find(|e| e.action == AuditAction::ChangePassword)
            .unwrap()
            .revoked_count,
        1
    );
    Ok(())
}

/// PostgreSQL is opt-in and every object lives in this test's random schema.
#[tokio::test]
async fn postgres_v2_upgrade_and_audit_failure_rollback_in_isolated_schema() -> Result<()> {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        return Ok(());
    };
    anyhow::ensure!(
        database_url.starts_with("postgres:") || database_url.starts_with("postgresql:"),
        "DATABASE_URL for the optional audit test must be PostgreSQL"
    );
    sqlx::any::install_default_drivers();
    let control = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    // The generated identifier contains only a fixed prefix and UUID hex digits.
    let schema = format!("audit_v3_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&control)
        .await?;
    let result = async {
        let mut scoped_url = url::Url::parse(&database_url)?;
        scoped_url
            .query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        postgres_v2_audit_contract(scoped_url.as_str(), &schema).await
    }
    .await;
    // Run cleanup even when the contract returns an error; never clear public tables.
    let cleanup = sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&control)
        .await;
    control.close().await;
    result?;
    cleanup?;
    Ok(())
}

async fn postgres_v2_audit_contract(database_url: &str, schema: &str) -> Result<()> {
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await?;
    let current_schema: String = sqlx::query_scalar("SELECT current_schema()::TEXT")
        .fetch_one(&pool)
        .await?;
    anyhow::ensure!(
        current_schema == schema,
        "PostgreSQL audit test schema isolation failed"
    );
    sqlx::raw_sql(include_str!("../migrations/postgres/0001_initial.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/postgres/0002_accounts.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(
        r#"CREATE TABLE account_schema_migrations(version BIGINT PRIMARY KEY NOT NULL);
        INSERT INTO account_schema_migrations(version) VALUES (1),(2);
        CREATE UNIQUE INDEX sessions_public_id_idx ON sessions(id);
        INSERT INTO users(id,username,display_name,role,source,password_hash) VALUES
            ('legacy-admin','legacy-admin','Administrator','admin','local','original-admin-hash'),
            ('legacy-member','legacy-member','Member','member','local','original-member-hash');
        INSERT INTO users(id,username,display_name,role,source,issuer,subject) VALUES
            ('legacy-oidc','legacy-oidc','OIDC member','member','oidc','https://test.example','subject');
        INSERT INTO books(user_id,snapshot) VALUES
            ('legacy-admin','{"peers":[],"tags":[]}'),
            ('legacy-member','{"peers":[{"id":"preserved-peer"}],"tags":["preserved-tag"]}'),
            ('legacy-oidc','{"peers":[],"tags":[]}');
        UPDATE settings SET value='1' WHERE key='bootstrap_initialized';"#,
    )
    .execute(&pool)
    .await?;
    let admin_token = "a".repeat(64);
    let member_token = "b".repeat(64);
    let oidc_token = "c".repeat(64);
    for (token, user, kind, public_id) in [
        (
            &admin_token,
            "legacy-admin",
            "web",
            "preserved-admin-session",
        ),
        (
            &member_token,
            "legacy-member",
            "client",
            "preserved-member-session",
        ),
        (
            &oidc_token,
            "legacy-oidc",
            "client",
            "preserved-oidc-session",
        ),
    ] {
        sqlx::query("INSERT INTO sessions(digest,id,user_id,kind,created_at,expires_at) VALUES ($1,$2,$3,$4,123,$5)")
            .bind(hex::encode(Sha256::digest(token.as_bytes())))
            .bind(public_id).bind(user).bind(kind).bind(chrono::Utc::now().timestamp() + 3600)
            .execute(&pool).await?;
    }
    let (store, peer) =
        tokio::try_join!(Store::connect(database_url), Store::connect(database_url))?;
    store.readiness().await?;
    peer.readiness().await?;
    anyhow::ensure!(
        sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM account_schema_migrations")
            .fetch_one(&pool)
            .await?
            == 4,
        "v2 database did not migrate to v4"
    );
    anyhow::ensure!(
        store.get_book_authenticated(&member_token).await?
            == json!({"peers":[{"id":"preserved-peer"}],"tags":["preserved-tag"]}),
        "v2 address book or client credential was not preserved"
    );
    let sessions = store
        .list_sessions(&admin_token, Some("legacy-member"))
        .await?;
    anyhow::ensure!(
        sessions.len() == 1
            && sessions[0].id == "preserved-member-session"
            && sessions[0].created_at == 123,
        "v2 public session identity or creation time changed"
    );
    // PostgreSQL aborts the entire transaction after this trigger raises; the
    // Store must roll back both its account/session writes and the failed audit.
    sqlx::raw_sql(
        "CREATE FUNCTION reject_audit_insert() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN RAISE EXCEPTION 'injected audit insert failure'; END;
         $$;
         CREATE TRIGGER reject_audit BEFORE INSERT ON audit_events
            FOR EACH ROW EXECUTE FUNCTION reject_audit_insert();",
    )
    .execute(&pool)
    .await?;
    anyhow::ensure!(
        store
            .admin_update_user(&admin_token, "legacy-member", Some(true), None)
            .await
            .is_err(),
        "disable unexpectedly succeeded when audit insert failed"
    );
    anyhow::ensure!(
        store
            .admin_reset_password(&admin_token, "legacy-member", "replacement-test-password")
            .await
            .is_err(),
        "password reset unexpectedly succeeded when audit insert failed"
    );
    anyhow::ensure!(
        store.sync_policy_version("rejected-policy").await.is_err(),
        "policy update unexpectedly succeeded when audit insert failed"
    );
    let user =
        sqlx::query("SELECT disabled,admitted,password_hash FROM users WHERE id='legacy-member'")
            .fetch_one(&pool)
            .await?;
    anyhow::ensure!(
        user.try_get::<i64, _>("disabled")? == 0
            && user.try_get::<i64, _>("admitted")? == 1
            && user.try_get::<String, _>("password_hash")? == "original-member-hash",
        "audit failure committed a disable or password change"
    );
    anyhow::ensure!(
        store.authenticate(&member_token).await?.is_some()
            && store.authenticate(&oidc_token).await?.is_some(),
        "audit failure committed member or policy session revocation"
    );
    anyhow::ensure!(
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='policy_version'")
            .fetch_one(&pool)
            .await?
            .is_empty()
            && sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_events")
                .fetch_one(&pool)
                .await?
                == 0,
        "audit failure committed policy state or an audit row"
    );
    pool.close().await;
    Ok(())
}
