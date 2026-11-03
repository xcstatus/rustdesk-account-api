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
    web_auth::{WebAuth, csrf_token},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:21114";
const PASSWORD: &str = "initial-password-123";

async fn raw_request(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Value,
) -> Result<(StatusCode, HeaderMap, Value)> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string()))?)
        .await?;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await?.to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, headers, body))
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> Result<(StatusCode, HeaderMap, Value)> {
    raw_request(
        app,
        method,
        path,
        &[
            ("cookie", format!("rd_web_session={token}")),
            ("origin", ORIGIN.into()),
            ("x-csrf-token", csrf_token(token)),
        ],
        body,
    )
    .await
}

async fn login(store: &Store, username: &str, password: &str, kind: SessionKind) -> Result<String> {
    Ok(store
        .login_local(username, password, 3600, kind)
        .await?
        .context("test account failed to log in")?
        .1)
}

async fn assert_revoked(app: &Router, token: &str) -> Result<()> {
    assert_eq!(
        request(app, "GET", "/admin/api/v1/sessions", token, Value::Null)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn management_contract_sqlite_and_optional_postgres() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let sqlite = format!(
        "sqlite://{}",
        directory.path().join("admin.sqlite").display()
    );
    management_contract(&sqlite).await?;
    if let Ok(url) = std::env::var("DATABASE_URL") {
        ensure!(
            url.starts_with("postgres:") || url.starts_with("postgresql:"),
            "DATABASE_URL for the optional admin test must be a dedicated PostgreSQL database"
        );
        management_contract(&url).await?;
    }
    Ok(())
}

async fn management_contract(url: &str) -> Result<()> {
    // Unique fixtures allow reruns without deleting pre-existing database state.
    let namespace = Uuid::new_v4().to_string();
    let store = Store::connect(url).await?;
    let admin_name = format!("admin-{namespace}");
    let admin_user = store.create_local(&admin_name, PASSWORD, "admin").await?;
    let admin_token = login(&store, &admin_name, PASSWORD, SessionKind::Web).await?;
    let admin_client = login(&store, &admin_name, PASSWORD, SessionKind::Client).await?;
    let app = admin::router(WebAuth::new(store.clone(), 3600, ORIGIN)?);
    let users_path = "/admin/api/v1/users";

    for headers in [
        vec![],
        vec![("authorization", format!("Bearer {admin_token}"))],
        vec![("cookie", format!("rd_web_session={admin_client}"))],
    ] {
        assert_eq!(
            raw_request(&app, "GET", users_path, &headers, Value::Null)
                .await?
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(store.authenticate(&admin_token).await?.is_none());

    let member_name = format!("member-{namespace}");
    let create_body = json!({"username":member_name,"password":PASSWORD,"role":"member"});
    let (status, headers, created) =
        request(&app, "POST", users_path, &admin_token, create_body.clone()).await?;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers["cache-control"], "no-store");
    let member_id = created["user"]["id"]
        .as_str()
        .context("created user id missing")?
        .to_owned();
    assert!(created["user"].get("password").is_none());
    let member_token = login(&store, &member_name, PASSWORD, SessionKind::Web).await?;
    let member_client = login(&store, &member_name, PASSWORD, SessionKind::Client).await?;
    let member_path = format!("{users_path}/{member_id}");
    let member_sessions = format!("{member_path}/sessions");
    let member_password = format!("{member_path}/password");

    for (method, path, body) in [
        ("GET", users_path, Value::Null),
        ("POST", users_path, create_body.clone()),
        ("PATCH", member_path.as_str(), json!({"disabled":true})),
        (
            "POST",
            member_password.as_str(),
            json!({"new_password":"replacement-password"}),
        ),
        ("GET", member_sessions.as_str(), Value::Null),
        ("DELETE", member_sessions.as_str(), Value::Null),
    ] {
        assert_eq!(
            request(&app, method, path, &member_token, body).await?.0,
            StatusCode::FORBIDDEN,
            "member access to {method} {path}"
        );
    }

    // Each mutation must reject a valid cookie without CSRF before changing state.
    let admin_sessions = request(
        &app,
        "GET",
        "/admin/api/v1/sessions",
        &admin_token,
        Value::Null,
    )
    .await?
    .2;
    let current_session = admin_sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["current"] == true)
        .context("current session missing")?["id"]
        .as_str()
        .unwrap();
    let current_session_path = format!("/admin/api/v1/sessions/{current_session}");
    for (method, path, body) in [
        ("POST", users_path, create_body.clone()),
        ("PATCH", member_path.as_str(), json!({"disabled":true})),
        (
            "POST",
            member_password.as_str(),
            json!({"new_password":"replacement-password"}),
        ),
        (
            "POST",
            "/admin/api/v1/me/password",
            json!({"current_password":PASSWORD,"new_password":"replacement-password"}),
        ),
        ("DELETE", "/admin/api/v1/sessions", Value::Null),
        ("DELETE", current_session_path.as_str(), Value::Null),
        ("DELETE", member_sessions.as_str(), Value::Null),
    ] {
        assert_eq!(
            raw_request(
                &app,
                method,
                path,
                &[
                    ("cookie", format!("rd_web_session={admin_token}")),
                    ("origin", ORIGIN.into())
                ],
                body
            )
            .await?
            .0,
            StatusCode::FORBIDDEN,
            "CSRF for {method} {path}"
        );
    }
    for (origin, csrf) in [
        ("https://attacker.example", csrf_token(&admin_token)),
        (ORIGIN, "incorrect-token".into()),
    ] {
        assert_eq!(
            raw_request(
                &app,
                "POST",
                users_path,
                &[
                    ("cookie", format!("rd_web_session={admin_token}")),
                    ("origin", origin.into()),
                    ("x-csrf-token", csrf)
                ],
                create_body.clone()
            )
            .await?
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        raw_request(
            &app,
            "POST",
            users_path,
            &[
                ("cookie", format!("rd_web_session={admin_token}")),
                ("x-csrf-token", csrf_token(&admin_token)),
            ],
            create_body.clone(),
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        request(&app, "POST", users_path, &admin_token, create_body)
            .await?
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            users_path,
            &admin_token,
            json!({"username":format!("bad-{namespace}"),"password":PASSWORD,"role":"owner"})
        )
        .await?
        .0,
        StatusCode::BAD_REQUEST
    );
    let listed = request(&app, "GET", users_path, &admin_token, Value::Null)
        .await?
        .2;
    let listed_member = listed["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == member_id)
        .context("member not listed")?;
    assert_eq!(listed_member["username"], member_name);
    assert_eq!(listed_member["source"], "local");
    assert_eq!(listed_member["admitted"], true);
    assert_eq!(listed_member["disabled"], false);
    assert!(listed_member.get("password_hash").is_none());

    // Promotion persists; a later disable revokes both browser and client sessions.
    assert_eq!(
        request(
            &app,
            "PATCH",
            &member_path,
            &admin_token,
            json!({"role":"admin"})
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", users_path, &member_token, Value::Null)
            .await?
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            &member_path,
            &admin_token,
            json!({"disabled":true})
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_revoked(&app, &member_token).await?;
    assert!(store.authenticate(&member_client).await?.is_none());
    assert!(
        store
            .login_local(&member_name, PASSWORD, 3600, SessionKind::Web)
            .await?
            .is_none()
    );
    assert_eq!(
        request(
            &app,
            "PATCH",
            &member_path,
            &admin_token,
            json!({"disabled":false,"role":"member"})
        )
        .await?
        .0,
        StatusCode::OK
    );
    let member_token = login(&store, &member_name, PASSWORD, SessionKind::Web).await?;
    let member_client = login(&store, &member_name, PASSWORD, SessionKind::Client).await?;

    let own = request(
        &app,
        "GET",
        "/admin/api/v1/sessions",
        &member_token,
        Value::Null,
    )
    .await?
    .2;
    let sessions = own["sessions"].as_array().unwrap();
    assert!(sessions.iter().all(|s| s["user_id"] == member_id));
    assert!(
        sessions
            .iter()
            .any(|s| s["kind"] == "web" && s["current"] == true)
    );
    assert!(sessions.iter().any(|s| s["kind"] == "client"));
    assert!(
        sessions
            .iter()
            .all(|s| s.get("digest").is_none() && s.get("token").is_none())
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &current_session_path,
            &member_token,
            Value::Null
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    let client_id = sessions
        .iter()
        .find(|s| s["kind"] == "client" && s["revoked"] == false)
        .context("active client session missing")?["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/admin/api/v1/sessions/{client_id}"),
            &member_token,
            Value::Null
        )
        .await?
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(store.authenticate(&member_client).await?.is_none());

    // Administrators may revoke a selected session belonging to another user.
    let other = login(&store, &member_name, PASSWORD, SessionKind::Web).await?;
    let other_list = request(&app, "GET", "/admin/api/v1/sessions", &other, Value::Null)
        .await?
        .2;
    let other_id = other_list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["current"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        request(
            &app,
            "DELETE",
            &format!("/admin/api/v1/sessions/{other_id}"),
            &admin_token,
            Value::Null
        )
        .await?
        .0,
        StatusCode::NO_CONTENT
    );
    assert_revoked(&app, &other).await?;
    let managed = request(&app, "GET", &member_sessions, &admin_token, Value::Null)
        .await?
        .2;
    assert!(
        managed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["user_id"] == member_id)
    );

    // An administrator reset invalidates every target session, including client tokens.
    let member_client = login(&store, &member_name, PASSWORD, SessionKind::Client).await?;
    assert_eq!(
        request(
            &app,
            "POST",
            &member_password,
            &admin_token,
            json!({"new_password":"reset-password-456"})
        )
        .await?
        .0,
        StatusCode::NO_CONTENT
    );
    assert_revoked(&app, &member_token).await?;
    assert!(store.authenticate(&member_client).await?.is_none());
    assert!(
        store
            .login_local(&member_name, PASSWORD, 3600, SessionKind::Web)
            .await?
            .is_none()
    );
    let member_token = login(&store, &member_name, "reset-password-456", SessionKind::Web).await?;
    let member_client = login(
        &store,
        &member_name,
        "reset-password-456",
        SessionKind::Client,
    )
    .await?;
    let wrong = request(
        &app,
        "POST",
        "/admin/api/v1/me/password",
        &member_token,
        json!({"current_password":"wrong-password-123","new_password":"changed-password-789"}),
    )
    .await?
    .0;
    assert_eq!(wrong, StatusCode::UNAUTHORIZED);
    assert_eq!(
        request(
            &app,
            "GET",
            "/admin/api/v1/sessions",
            &member_token,
            Value::Null
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/admin/api/v1/me/password",
            &member_token,
            json!({"current_password":"reset-password-456","new_password":"changed-password-789"})
        )
        .await?
        .0,
        StatusCode::NO_CONTENT
    );
    assert_revoked(&app, &member_token).await?;
    assert!(store.authenticate(&member_client).await?.is_none());
    let member_token = login(
        &store,
        &member_name,
        "changed-password-789",
        SessionKind::Web,
    )
    .await?;

    // Provider-managed credentials and roles cannot be replaced by local administration.
    let oidc = store
        .find_or_create_oidc(
            &format!("https://id.example/{namespace}"),
            "subject",
            "Federated member",
            Some("member"),
        )
        .await?
        .unwrap();
    let oidc_path = format!("{users_path}/{}", oidc.id);
    assert_eq!(
        request(
            &app,
            "PATCH",
            &oidc_path,
            &admin_token,
            json!({"role":"admin"})
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("{oidc_path}/password"),
            &admin_token,
            json!({"new_password":"federated-password"})
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    let oidc_token = store.issue_web_session(&oidc.id, 3600).await?;
    assert_eq!(
        request(
            &app,
            "POST",
            "/admin/api/v1/me/password",
            &oidc_token,
            json!({"current_password":PASSWORD,"new_password":"federated-password"}),
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("{users_path}/{}/sessions", Uuid::new_v4()),
            &admin_token,
            Value::Null,
        )
        .await?
        .0,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        request(&app, "DELETE", &member_sessions, &admin_token, Value::Null)
            .await?
            .0,
        StatusCode::NO_CONTENT
    );
    assert_revoked(&app, &member_token).await?;
    let member_token = login(
        &store,
        &member_name,
        "changed-password-789",
        SessionKind::Web,
    )
    .await?;
    let member_client = login(
        &store,
        &member_name,
        "changed-password-789",
        SessionKind::Client,
    )
    .await?;
    assert_eq!(
        request(
            &app,
            "DELETE",
            "/admin/api/v1/sessions",
            &member_token,
            Value::Null
        )
        .await?
        .0,
        StatusCode::NO_CONTENT
    );
    assert_revoked(&app, &member_token).await?;
    assert!(store.authenticate(&member_client).await?.is_none());
    assert_eq!(
        request(&app, "GET", users_path, &admin_token, Value::Null)
            .await?
            .0,
        StatusCode::OK
    );
    assert_eq!(admin_user.role, "admin");
    Ok(())
}

#[tokio::test]
async fn last_enabled_local_administrator_is_preserved() -> Result<()> {
    let store = Store::connect("sqlite::memory:").await?;
    let admin = store.bootstrap_admin("bootstrap-admin", PASSWORD).await?;
    let token = login(&store, "bootstrap-admin", PASSWORD, SessionKind::Web).await?;
    let app = admin::router(WebAuth::new(store.clone(), 3600, ORIGIN)?);
    let path = format!("/admin/api/v1/users/{}", admin.id);
    // An OIDC administrator does not satisfy the local recovery-account invariant.
    store
        .find_or_create_oidc(
            "https://id.example/last-admin",
            "subject",
            "Federated admin",
            Some("admin"),
        )
        .await?;
    for body in [json!({"disabled":true}), json!({"role":"member"})] {
        assert_eq!(
            request(&app, "PATCH", &path, &token, body).await?.0,
            StatusCode::CONFLICT
        );
    }
    let (status, _, second) = request(
        &app,
        "POST",
        "/admin/api/v1/users",
        &token,
        json!({"username":"second-admin","password":PASSWORD,"role":"admin"}),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        request(&app, "PATCH", &path, &token, json!({"role":"member"}))
            .await?
            .0,
        StatusCode::OK
    );
    assert_revoked(&app, &token).await?;
    let token = login(&store, "second-admin", PASSWORD, SessionKind::Web).await?;
    let path = format!(
        "/admin/api/v1/users/{}",
        second["user"]["id"].as_str().unwrap()
    );
    assert_eq!(
        request(&app, "PATCH", &path, &token, json!({"disabled":true}))
            .await?
            .0,
        StatusCode::CONFLICT
    );
    Ok(())
}
