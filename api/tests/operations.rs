use anyhow::Result;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    middleware,
};
use rustdesk_account_api::{
    compat,
    limits::{LoginLimitConfig, LoginLimiter},
    operations,
    store::Store,
    web_auth::{self, WebAuth},
};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn web_and_client_share_login_limit_and_forwarded_headers_cannot_bypass() -> Result<()> {
    let store = Store::connect("sqlite::memory:").await?;
    store
        .create_local("member", "real-test-password", "member")
        .await?;
    let limiter = LoginLimiter::new(LoginLimitConfig {
        per_account: 2,
        global: 5,
        ..Default::default()
    })?;
    let auth = WebAuth::new(store.clone(), 3600, "http://127.0.0.1:21114")?
        .with_login_limiter(limiter.clone());
    let app =
        compat::router_with_limiter(store, 3600, None, limiter).merge(web_auth::router(auth, None));
    for (path, expected) in [
        ("/admin/api/v1/auth/login", StatusCode::UNAUTHORIZED),
        ("/api/login", StatusCode::UNAUTHORIZED),
        ("/api/login", StatusCode::TOO_MANY_REQUESTS),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("origin", "http://127.0.0.1:21114")
            .header("x-forwarded-for", "1.2.3.4")
            .header("forwarded", "for=9.8.7.6;proto=https")
            .body(Body::from(
                json!({"username":"member","password":"incorrect-password","type":"account"})
                    .to_string(),
            ))?;
        assert_eq!(app.clone().oneshot(request).await?.status(), expected);
    }
    Ok(())
}

#[tokio::test]
async fn readiness_is_separate_from_liveness_and_logs_exclude_request_secrets() -> Result<()> {
    use std::sync::{Arc, Mutex};
    use tracing::instrument::WithSubscriber;
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let store = Store::connect("sqlite::memory:").await?;
    let directory = tempfile::tempdir()?;
    let app: Router = operations::router(store.clone(), directory.path().to_path_buf())
        .merge(compat::router(store, 3600, None))
        .layer(middleware::from_fn(operations::log_request));
    assert_eq!(
        app.clone()
            .oneshot(Request::builder().uri("/healthz").body(Body::empty())?)
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone()
            .oneshot(Request::builder().uri("/readyz").body(Body::empty())?)
            .await?
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    std::fs::write(directory.path().join("index.html"), "ready")?;
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/readyz?code=synthetic-secret-query")
                .header("authorization", "Bearer synthetic-secret-token")
                .body(Body::from("synthetic-secret-body"))?,
        )
        .with_subscriber(subscriber)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let log = String::from_utf8(output.lock().unwrap().clone())?;
    assert!(log.contains("route=/readyz") && log.contains("status=200"));
    assert!(!log.contains("synthetic-secret"));
    Ok(())
}
