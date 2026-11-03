use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use http_body_util::BodyExt;
use rustdesk_account_api::{
    admin,
    store::{SessionKind, Store},
    web_auth::WebAuth,
};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:21114";
const PASSWORD: &str = "audit-test-password-123";
const PATH: &str = "/admin/api/v1/audit-events";

async fn get(
    app: &Router,
    path: &str,
    headers: &[(&str, String)],
) -> Result<(StatusCode, HeaderMap, Value)> {
    let mut request = Request::builder().uri(path);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = app.clone().oneshot(request.body(Body::empty())?).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await?.to_bytes();
    // Axum query rejections may be plain text rather than the account error JSON.
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    Ok((status, headers, body))
}

async fn as_web(app: &Router, query: &str, token: &str) -> Result<(StatusCode, HeaderMap, Value)> {
    get(
        app,
        &format!("{PATH}{query}"),
        &[("cookie", format!("rd_web_session={token}"))],
    )
    .await
}

async fn login(store: &Store, username: &str, kind: SessionKind) -> Result<String> {
    Ok(store
        .login_local(username, PASSWORD, 3600, kind)
        .await?
        .context("audit fixture login failed")?
        .1)
}

#[tokio::test]
async fn audit_contract_sqlite_and_optional_postgres() -> Result<()> {
    let directory = tempfile::tempdir()?;
    audit_contract(&format!(
        "sqlite://{}",
        directory.path().join("audit-api.sqlite").display()
    ))
    .await?;
    if let Ok(url) = std::env::var("DATABASE_URL") {
        ensure!(
            url.starts_with("postgres:") || url.starts_with("postgresql:"),
            "DATABASE_URL must be a dedicated PostgreSQL test database"
        );
        audit_contract(&url).await?;
    }
    Ok(())
}

async fn audit_contract(url: &str) -> Result<()> {
    let namespace = Uuid::new_v4();
    let store = Store::connect(url).await?;
    let admin_name = format!("audit-admin-{namespace}");
    let member_name = format!("audit-member-{namespace}");
    let admin_user = store.create_local(&admin_name, PASSWORD, "admin").await?;
    let member = store.create_local(&member_name, PASSWORD, "member").await?;
    let admin_token = login(&store, &admin_name, SessionKind::Web).await?;
    let admin_client = login(&store, &admin_name, SessionKind::Client).await?;
    let member_token = login(&store, &member_name, SessionKind::Web).await?;
    let app = admin::router(WebAuth::new(store.clone(), 3600, ORIGIN)?);

    // A client-purpose token cannot become a Web session, including in a cookie.
    for headers in [
        vec![],
        vec![("authorization", format!("Bearer {admin_token}"))],
        vec![("authorization", format!("Bearer {admin_client}"))],
        vec![("cookie", format!("rd_web_session={admin_client}"))],
    ] {
        assert_eq!(get(&app, PATH, &headers).await?.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        as_web(&app, "", &member_token).await?.0,
        StatusCode::FORBIDDEN
    );

    store
        .admin_update_user(&admin_token, &member.id, Some(true), None)
        .await?;
    store
        .admin_update_user(&admin_token, &member.id, Some(false), None)
        .await?;
    let until = chrono::Utc::now().timestamp() + 60;
    let filter = format!(
        "target_id={}&actor_id={}&action=update_user&source=local&kind=web&result=success&since=0&until={until}&limit=1",
        member.id, admin_user.id
    );
    let (status, headers, first) = as_web(&app, &format!("?{filter}"), &admin_token).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    let first_events = first["events"].as_array().context("events array missing")?;
    assert_eq!(first_events.len(), 1);
    let event = &first_events[0];
    assert_eq!(event["actor_id"], admin_user.id);
    assert_eq!(event["target_id"], member.id);
    assert_eq!(event["source"], "local");
    assert_eq!(event["kind"], "web");
    assert_eq!(event["action"], "update_user");
    assert_eq!(event["result"], "success");
    assert_eq!(event["before_disabled"], true);
    assert_eq!(event["after_disabled"], false);
    assert!(event["timestamp"].as_i64().is_some());
    assert!(event["revoked_count"].as_i64().is_some());
    assert!(event["policy_version"].as_str().is_some());
    for key in [
        "password",
        "password_hash",
        "token",
        "access_token",
        "claims",
        "username",
        "display_name",
    ] {
        assert!(event.get(key).is_none(), "unexpected audit field {key}");
    }
    let cursor = first["next_before_id"].as_i64().context("cursor missing")?;
    assert_eq!(Some(cursor), event["id"].as_i64());

    // A newer write between pages must not reappear below the previous cursor.
    store
        .admin_update_user(&admin_token, &member.id, Some(true), None)
        .await?;
    let (status, _, second) =
        as_web(&app, &format!("?{filter}&before_id={cursor}"), &admin_token).await?;
    assert_eq!(status, StatusCode::OK);
    let second_events = second["events"].as_array().context("second page missing")?;
    assert_eq!(second_events.len(), 1);
    assert!(second_events[0]["id"].as_i64().unwrap() < cursor);
    assert_eq!(second_events[0]["before_disabled"], false);
    assert_eq!(second_events[0]["after_disabled"], true);
    assert!(second["next_before_id"].is_null());

    // Literal target identifiers are bound values, never SQL expressions.
    let (_, _, empty) = as_web(&app, "?target_id=%27%20OR%201%3D1--", &admin_token).await?;
    assert_eq!(empty["events"].as_array().unwrap().len(), 0);
    assert!(empty["next_before_id"].is_null());
    let (_, _, empty) = as_web(
        &app,
        &format!("?target_id={}&since={until}", member.id),
        &admin_token,
    )
    .await?;
    assert!(empty["events"].as_array().unwrap().is_empty());

    store
        .record_login_failure("local", Some(SessionKind::Web), "invalid_credentials")
        .await?;
    let (status, _, failures) = as_web(
        &app,
        "?action=login&source=local&kind=web&result=failure",
        &admin_token,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let failures = failures["events"].as_array().unwrap();
    assert!(!failures.is_empty());
    for event in failures {
        assert_eq!(event["result"], "failure");
        assert_eq!(event["action"], "login");
        assert_eq!(event["source"], "local");
        assert_eq!(event["kind"], "web");
    }
    assert!(
        failures
            .iter()
            .any(|event| event["reason"] == "invalid_credentials")
    );

    // Both extraction errors and Store validation errors must be HTTP 400.
    let too_long = format!("?target_id={}", "x".repeat(129));
    for query in [
        "?limit=0",
        "?limit=201",
        "?limit=-1",
        "?limit=lots",
        "?limit=1&limit=2",
        "?before_id=0",
        "?before_id=-1",
        "?before_id=9223372036854775808",
        "?since=3&until=2",
        "?since=invalid",
        "?action=download",
        "?source=other",
        "?kind=admin",
        "?result=unknown",
        "?unknown=true",
        &too_long,
    ] {
        assert_eq!(
            as_web(&app, query, &admin_token).await?.0,
            StatusCode::BAD_REQUEST,
            "query {query}"
        );
    }
    // Demoting the current administrator invalidates its existing Web session.
    store
        .create_local(&format!("audit-spare-{namespace}"), PASSWORD, "admin")
        .await?;
    store
        .admin_update_user(&admin_token, &admin_user.id, None, Some("member"))
        .await?;
    assert_eq!(
        as_web(&app, "", &admin_token).await?.0,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}
