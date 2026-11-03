//! Operational probes and request logging without credentials or URL queries.
use crate::store::Store;
use axum::{
    Router,
    extract::{MatchedPath, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use std::{path::PathBuf, time::Instant};

#[derive(Clone)]
struct Probes {
    store: Store,
    web_dir: PathBuf,
}
pub async fn ready(store: &Store, web_dir: &std::path::Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        tokio::fs::metadata(web_dir.join("index.html"))
            .await?
            .is_file(),
        "Web build missing"
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), store.readiness()).await??;
    Ok(())
}
pub fn router(store: Store, web_dir: PathBuf) -> Router {
    Router::new()
        .route("/readyz", get(readiness))
        .with_state(Probes { store, web_dir })
}
async fn readiness(State(state): State<Probes>) -> Response {
    if ready(&state.store, &state.web_dir).await.is_ok() {
        (StatusCode::OK, "ready").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response()
    }
}
pub async fn log_request(request: Request, next: Next) -> Response {
    // MatchedPath is a route template; URI/query, headers, bodies and peer IP are never logged.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    let method = request.method().clone();
    let started = Instant::now();
    let response = next.run(request).await;
    tracing::info!(method = %method, route = %route, status = response.status().as_u16(), elapsed_ms = started.elapsed().as_millis() as u64, "request");
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn readiness_requires_web_build_and_live_schema() -> anyhow::Result<()> {
        let store = Store::connect("sqlite::memory:").await?;
        let dir = tempfile::tempdir()?;
        assert!(ready(&store, dir.path()).await.is_err());
        std::fs::write(dir.path().join("index.html"), "ready")?;
        ready(&store, dir.path()).await?;
        Ok(())
    }
}
