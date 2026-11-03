use anyhow::Result;
use rustdesk_account_api::store::{DeviceLogin, SessionKind, Store};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};

const PASSWORD: &str = "device-test-password";

async fn contract(url: &str) -> Result<()> {
    let store = Store::connect(url).await?;
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await?;
    let admin = store.bootstrap_admin("admin", PASSWORD).await?;
    let admin_web = store.issue_web_session(&admin.id, 3600).await?;
    let member = store
        .admin_create_local(&admin_web, "member", PASSWORD, "member")
        .await?;
    let member_web = store.issue_web_session(&member.id, 3600).await?;
    let ordinary = store.issue_session(&member.id, 3600).await?;
    assert!(
        store
            .list_sessions(&member_web, None)
            .await?
            .iter()
            .all(|s| s.device_id.is_none())
    );
    let device = DeviceLogin {
        rustdesk_id: "123456789".into(),
        uuid: "private-self-reported-device-uuid".into(),
        ..Default::default()
    };
    let token = store
        .login_local_with_device("member", PASSWORD, 3600, SessionKind::Client, Some(&device))
        .await?
        .unwrap()
        .1;
    let listed = store
        .list_devices(&token, SessionKind::Client, false)
        .await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].owner_user_id.as_deref(), Some(member.id.as_str()));
    assert_eq!(listed[0].owner_username.as_deref(), Some("member"));
    assert!(listed[0].last_login_at > 0);
    let id = listed[0].id.clone();
    assert!(!serde_json::to_string(&listed)?.contains(&device.uuid));
    assert!(
        store
            .list_devices(&ordinary, SessionKind::Client, true)
            .await
            .is_err()
    );
    assert!(
        store
            .list_devices(&member_web, SessionKind::Web, true)
            .await
            .is_err()
    );
    assert!(
        store
            .assign_device(&member_web, &id, Some(&admin.id))
            .await
            .is_err()
    );
    assert!(
        store
            .set_device_alias(&member_web, &id, "forbidden")
            .await
            .is_err()
    );

    // Another account can sign in from this machine; its login cannot take ownership.
    let other = store
        .issue_session_with_device(&admin.id, 3600, Some(&device))
        .await?;
    assert!(store.authenticate(&other).await?.is_some());
    assert!(
        store
            .list_devices(&other, SessionKind::Client, false)
            .await?
            .is_empty()
    );
    assert!(
        store
            .list_sessions(&admin_web, Some(&admin.id))
            .await?
            .iter()
            .all(|s| s.device_id.is_none())
    );
    let conflict = DeviceLogin {
        uuid: "different-uuid".into(),
        ..device.clone()
    };
    let conflict_token = store
        .issue_session_with_device(&admin.id, 3600, Some(&conflict))
        .await?;
    assert!(store.authenticate(&conflict_token).await?.is_some());
    assert_eq!(
        store
            .list_devices(&admin_web, SessionKind::Web, true)
            .await?
            .len(),
        1
    );
    let invalid = DeviceLogin {
        uuid: "x".repeat(257),
        ..device.clone()
    };
    store
        .issue_session_with_device(&member.id, 3600, Some(&invalid))
        .await?;
    assert_eq!(
        store
            .list_devices(&member_web, SessionKind::Web, false)
            .await?
            .len(),
        1
    );

    // A stable UUID owned by this user can follow a changed, unoccupied RD ID.
    let moved = DeviceLogin {
        rustdesk_id: "987654321".into(),
        name: "test laptop".into(),
        ..device.clone()
    };
    let moved_token = store
        .issue_session_with_device(&member.id, 3600, Some(&moved))
        .await?;
    let devices = store
        .list_devices(&member_web, SessionKind::Web, false)
        .await?;
    assert_eq!(devices[0].id, id);
    assert_eq!(devices[0].rustdesk_id, moved.rustdesk_id);
    assert_eq!(devices[0].device_name, "test laptop");
    let partial = DeviceLogin {
        name: String::new(),
        ..moved.clone()
    };
    store
        .issue_session_with_device(&member.id, 3600, Some(&partial))
        .await?;
    assert_eq!(
        store
            .list_devices(&member_web, SessionKind::Web, false)
            .await?[0]
            .device_name,
        "test laptop"
    );
    store.logout_session(&token, SessionKind::Client).await?;
    assert_eq!(
        store
            .list_devices(&member_web, SessionKind::Web, false)
            .await?
            .len(),
        1
    );
    store.set_device_alias(&admin_web, &id, "办公机").await?;

    // Audit failure must roll back ownership and session revocation together.
    if url.starts_with("sqlite:") {
        sqlx::raw_sql("CREATE TRIGGER reject_device_event BEFORE INSERT ON device_events BEGIN SELECT RAISE(ABORT, 'test rejection'); END;")
            .execute(&pool).await?;
    } else {
        sqlx::raw_sql("CREATE FUNCTION reject_device_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test rejection'; END; $$; CREATE TRIGGER reject_device_event BEFORE INSERT ON device_events FOR EACH ROW EXECUTE FUNCTION reject_device_event();")
            .execute(&pool).await?;
    }
    assert!(
        store
            .assign_device(&admin_web, &id, Some(&admin.id))
            .await
            .is_err()
    );
    assert!(store.authenticate(&moved_token).await?.is_some());
    assert_eq!(
        store
            .list_devices(&member_web, SessionKind::Web, false)
            .await?
            .len(),
        1
    );
    sqlx::query(if url.starts_with("sqlite:") {
        "DROP TRIGGER reject_device_event"
    } else {
        "DROP TRIGGER reject_device_event ON device_events"
    })
    .execute(&pool)
    .await?;
    store
        .assign_device(&admin_web, &id, Some(&admin.id))
        .await?;
    assert!(store.authenticate(&moved_token).await?.is_none());
    assert!(store.authenticate(&ordinary).await?.is_some());
    assert!(
        store
            .list_devices(&member_web, SessionKind::Web, false)
            .await?
            .is_empty()
    );
    let events = sqlx::query("SELECT action,revoked_count FROM device_events ORDER BY id")
        .fetch_all(&pool)
        .await?;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].try_get::<String, _>("action")?, "assign");
    assert!(events[1].try_get::<i64, _>("revoked_count")? >= 1);
    store.assign_device(&admin_web, &id, None).await?;
    let new_token = store
        .issue_session_with_device(&member.id, 3600, Some(&moved))
        .await?;
    assert!(
        store
            .list_devices(&new_token, SessionKind::Client, false)
            .await?
            .is_empty()
    );
    assert!(
        store
            .list_devices(&admin_web, SessionKind::Web, true)
            .await?[0]
            .owner_user_id
            .is_none()
    );
    let reopened = Store::connect(url).await?;
    assert_eq!(
        reopened
            .list_devices(&admin_web, SessionKind::Web, true)
            .await?[0]
            .alias,
        "办公机"
    );
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn sqlite_persistent_device_ownership_and_atomic_management() -> Result<()> {
    let directory = tempfile::tempdir()?;
    contract(&format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("devices.sqlite").display()
    ))
    .await
}

#[tokio::test]
async fn postgres_devices_in_isolated_schema() -> Result<()> {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        return Ok(());
    };
    anyhow::ensure!(
        database_url.starts_with("postgres:") || database_url.starts_with("postgresql:"),
        "optional device tests require PostgreSQL"
    );
    sqlx::any::install_default_drivers();
    let control: AnyPool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    let schema = format!("device_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&control)
        .await?;
    let result = async {
        let mut scoped = url::Url::parse(&database_url)?;
        scoped
            .query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        contract(scoped.as_str()).await
    }
    .await;
    let cleanup = sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&control)
        .await;
    control.close().await;
    result?;
    cleanup?;
    Ok(())
}
