use anyhow::Result;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use rustdesk_account_api::{
    compat,
    store::{SessionKind, Store},
    web_auth::{self, WebAuth},
    web_files,
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    origin: Option<&str>,
    csrf: Option<&str>,
    body: Value,
) -> Result<axum::response::Response> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(value) = cookie {
        request = request.header(header::COOKIE, value);
    }
    if let Some(value) = origin {
        request = request.header(header::ORIGIN, value);
    }
    if let Some(value) = csrf {
        request = request.header("x-csrf-token", value);
    }
    Ok(app
        .clone()
        .oneshot(request.body(Body::from(body.to_string()))?)
        .await?)
}

#[tokio::test]
async fn browser_login_cookie_csrf_origin_and_revocation() -> Result<()> {
    let store = Store::connect("sqlite::memory:").await?;
    store.bootstrap_admin("admin", "initial-password").await?;
    let auth = WebAuth::new(store.clone(), 28800, "http://127.0.0.1:21114")?;
    let app = web_auth::router(auth, None).merge(compat::router(store.clone(), 3600, None));
    let input = json!({"username":"admin", "password":"initial-password"});
    let origin = Some("http://127.0.0.1:21114");
    for bad in [None, Some("http://evil.example"), Some("null")] {
        assert_eq!(
            call(
                &app,
                "POST",
                "/admin/api/v1/auth/login",
                None,
                bad,
                None,
                input.clone()
            )
            .await?
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = call(
        &app,
        "POST",
        "/admin/api/v1/auth/login",
        None,
        origin,
        None,
        input,
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str()?.to_owned();
    assert!(
        set_cookie.contains("HttpOnly")
            && set_cookie.contains("SameSite=Lax")
            && set_cookie.contains("Path=/admin/api/v1")
    );
    let cookie = set_cookie.split(';').next().unwrap();
    let token = cookie.split_once('=').unwrap().1;
    let value: Value = serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert!(value.get("access_token").is_none());
    assert_eq!(value["user"]["source"], "local");
    let csrf = value["csrf_token"].as_str().unwrap();
    assert!(store.authenticate(token).await?.is_none());
    assert!(store.authenticate_web(token).await?.is_some());
    let (_, client) = store
        .login_local("admin", "initial-password", 3600, SessionKind::Client)
        .await?
        .unwrap();
    let wrong = format!("rd_web_session={client}");
    assert_eq!(
        call(
            &app,
            "GET",
            "/admin/api/v1/me",
            Some(&wrong),
            None,
            None,
            Value::Null
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/admin/api/v1/me",
            Some(cookie),
            None,
            None,
            Value::Null
        )
        .await?
        .status(),
        StatusCode::OK
    );
    for (bad_origin, bad_csrf) in [
        (origin, None),
        (None, Some(csrf)),
        (Some("http://evil.example"), Some(csrf)),
        (origin, Some("wrong")),
    ] {
        assert_eq!(
            call(
                &app,
                "POST",
                "/admin/api/v1/auth/logout",
                Some(cookie),
                bad_origin,
                bad_csrf,
                json!({})
            )
            .await?
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = call(
        &app,
        "POST",
        "/admin/api/v1/auth/logout",
        Some(cookie),
        origin,
        Some(csrf),
        json!({}),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()?
            .contains("Max-Age=0")
    );
    assert!(store.authenticate_web(token).await?.is_none());
    assert!(store.authenticate(&client).await?.is_some());
    assert_eq!(
        call(
            &app,
            "GET",
            "/admin/api/v1/me",
            Some(cookie),
            None,
            None,
            Value::Null
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn browser_config_and_cookie_parser_are_strict() -> Result<()> {
    let store = Store::connect("sqlite::memory:").await?;
    for origin in [
        "http://example.com",
        "https://u:p@example.com",
        "https://example.com/path",
        "https://example.com/?a=1",
        "https://example.com/#x",
    ] {
        assert!(WebAuth::new(store.clone(), 3600, origin).is_err());
    }
    let auth = WebAuth::new(store, 3600, "https://accounts.example")?;
    let secret = "a".repeat(64);
    assert!(auth.session_cookie(&secret).contains("; Secure"));
    assert!(
        auth.oidc_cookie(&secret)
            .contains("Path=/api/oidc/callback")
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("rd_web_session={secret}; rd_web_session={secret}").parse()?,
    );
    assert!(web_auth::cookie(&headers, "rd_web_session").is_none());
    Ok(())
}

#[tokio::test]
async fn static_ui_only_serves_files_within_selected_directory() -> Result<()> {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("index.html"), "<html>Test UI</html>")?;
    std::fs::write(root.path().join("secret.txt"), "not a public asset")?;
    let app = web_files::router(root.path().to_string_lossy().into_owned());
    assert_eq!(
        call(&app, "GET", "/admin/", None, None, None, Value::Null)
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/admin/secret.txt",
            None,
            None,
            None,
            Value::Null
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/admin/%2e%2e/secret.txt",
            None,
            None,
            None,
            Value::Null
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    #[cfg(unix)]
    {
        let external = tempfile::NamedTempFile::new()?;
        std::os::unix::fs::symlink(external.path(), root.path().join("outside.js"))?;
        assert_eq!(
            call(
                &app,
                "GET",
                "/admin/outside.js",
                None,
                None,
                None,
                Value::Null
            )
            .await?
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    Ok(())
}
