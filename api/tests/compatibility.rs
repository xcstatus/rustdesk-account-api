use anyhow::Result;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use rustdesk_account_api::{compat, store::Store};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> Result<(StatusCode, axum::http::HeaderMap, Value)> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string()))?)
        .await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await?.to_bytes();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    };
    Ok((status, headers, body))
}
async fn login(app: &Router, username: &str, password: &str) -> Result<Value> {
    let (status,headers,value)=request(app,"POST","/api/login",None,json!({"username":username,"password":password,"id":"device-a","uuid":"uuid-a","autoLogin":true,"type":"account"})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(value["type"], "access_token");
    assert_eq!(value["user"]["info"], json!({}));
    Ok(value)
}

#[tokio::test]
async fn local_login_book_roundtrip_isolation_and_revocation() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let url = format!("sqlite://{}", directory.path().join("api.sqlite").display());
    let store = Store::connect(&url).await?;
    store
        .create_local("alice", "alice-password-123", "member")
        .await?;
    store
        .create_local("bob", "bob-password-12345", "member")
        .await?;
    let app = compat::router(store.clone(), 3600, None);
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/login",
            None,
            json!({"username":"alice","password":"wrong","type":"account"})
        )
        .await?
        .0,
        StatusCode::UNAUTHORIZED
    );
    let alice = login(&app, "alice", "alice-password-123").await?;
    let alice_token = alice["access_token"].as_str().unwrap();
    let second = login(&app, "alice", "alice-password-123").await?;
    let second_token = second["access_token"].as_str().unwrap();
    assert_ne!(alice_token, second_token);
    let bob = login(&app, "bob", "bob-password-12345").await?;
    let bob_token = bob["access_token"].as_str().unwrap();
    assert_eq!(
        request(&app, "POST", "/api/ab/personal", None, Value::Null)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/ab/personal",
            Some(alice_token),
            Value::Null
        )
        .await?
        .0,
        StatusCode::NOT_FOUND
    );
    let current = request(
        &app,
        "POST",
        "/api/currentUser",
        Some(alice_token),
        json!({"user_id":bob["user"]["id"]}),
    )
    .await?
    .2;
    assert_eq!(current["id"], alice["user"]["id"]);
    assert!(
        current.get("user").is_none(),
        "currentUser must not add a wrapper"
    );
    let book = json!({"peers":[{"id":"123456789","alias":"测试设备","platform":"Mac OS","tags":["朋友"],"hash":"synthetic-remote-secret","password":"never-store"}],"tags":["朋友"],"tag_colors":"{\"朋友\":4281558681}","user_id":bob["user"]["id"]});
    let (status, headers, _) = request(
        &app,
        "POST",
        "/api/ab",
        Some(alice_token),
        json!({"data":book.to_string()}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers["x-rustdesk-credential-sync"],
        "disabled; metadata-only"
    );
    let read = request(&app, "GET", "/api/ab", Some(second_token), Value::Null)
        .await?
        .2;
    assert!(
        read["data"].is_string(),
        "Legacy data must be a JSON string"
    );
    let snapshot: Value = serde_json::from_str(read["data"].as_str().unwrap())?;
    assert_eq!(snapshot["peers"][0]["alias"], "测试设备");
    assert!(snapshot["peers"][0].get("hash").is_none());
    assert!(snapshot["peers"][0].get("password").is_none());
    assert!(snapshot.get("user_id").is_none());
    let other = request(&app, "GET", "/api/ab", Some(bob_token), Value::Null)
        .await?
        .2;
    let other: Value = serde_json::from_str(other["data"].as_str().unwrap())?;
    assert_eq!(other["peers"], json!([]));
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/ab",
            Some(alice_token),
            json!({"data":"{invalid"})
        )
        .await?
        .0,
        StatusCode::BAD_REQUEST
    );
    let unchanged = request(&app, "GET", "/api/ab", Some(alice_token), Value::Null)
        .await?
        .2;
    assert_eq!(unchanged, read);
    assert_eq!(
        request(&app, "POST", "/api/logout", Some(alice_token), Value::Null)
            .await?
            .0,
        StatusCode::OK
    );
    for (method, path) in [
        ("POST", "/api/currentUser"),
        ("GET", "/api/ab"),
        ("POST", "/api/ab/personal"),
        ("GET", "/api/users"),
    ] {
        assert_eq!(
            request(&app, method, path, Some(alice_token), Value::Null)
                .await?
                .0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    assert_eq!(
        request(&app, "GET", "/api/ab", Some(second_token), Value::Null)
            .await?
            .0,
        StatusCode::OK,
        "logout revokes the selected session"
    );
    let reconnected = Store::connect(&url).await?;
    assert!(reconnected.authenticate(alice_token).await?.is_none());
    assert_eq!(
        reconnected
            .get_book(alice["user"]["id"].as_str().unwrap())
            .await?,
        snapshot
    );
    store
        .disable_user(alice["user"]["id"].as_str().unwrap())
        .await?;
    assert_eq!(
        request(&app, "GET", "/api/ab", Some(second_token), Value::Null)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn address_book_clear_and_invalid_snapshot_do_not_corrupt_data() -> Result<()> {
    let store = Store::connect("sqlite::memory:").await?;
    let user = store
        .create_local("member", "safe-test-password", "member")
        .await?;
    let token = store.issue_session(&user.id, 3600).await?;
    let app = compat::router(store.clone(), 3600, None);
    let valid = json!({"peers":[{"id":"987654321","alias":"first","hash":""}],"tags":[]});
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/ab",
            Some(&token),
            json!({"data":valid.to_string()})
        )
        .await?
        .0,
        StatusCode::OK
    );
    for invalid in [
        json!({"peers":[{"id":3}],"tags":[]}),
        json!({"peers":[],"tags":[5]}),
        json!({"peers":[],"tags":[],"tag_colors":"null"}),
    ] {
        assert_eq!(
            request(
                &app,
                "POST",
                "/api/ab",
                Some(&token),
                json!({"data":invalid.to_string()})
            )
            .await?
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            store.get_book(&user.id).await?["peers"][0]["alias"],
            "first"
        );
    }
    let empty = json!({"peers":[],"tags":[]});
    assert_eq!(
        request(
            &app,
            "POST",
            "/api/ab",
            Some(&token),
            json!({"data":empty.to_string()})
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(store.get_book(&user.id).await?, empty);
    Ok(())
}
