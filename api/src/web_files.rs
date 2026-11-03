//! Serve the separately built management UI from an explicitly selected directory.
use axum::{
    Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use std::path::{Component, PathBuf};

pub fn router(directory: String) -> Router {
    Router::new()
        .route("/admin", get(|| async { Redirect::permanent("/admin/") }))
        .route("/admin/", get(index))
        .route("/admin/{*path}", get(asset))
        .with_state(PathBuf::from(directory))
}
async fn index(State(root): State<PathBuf>) -> Response {
    read(root, "index.html".into()).await
}
async fn asset(State(root): State<PathBuf>, Path(path): Path<String>) -> Response {
    read(root, path).await
}
async fn read(root: PathBuf, path: String) -> Response {
    if path.is_empty()
        || std::path::Path::new(&path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let root = match tokio::fs::canonicalize(root).await {
        Ok(root) => root,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Management UI is not built. Run npm ci && npm run build in web/.",
            )
                .into_response();
        }
    };
    let file = match tokio::fs::canonicalize(root.join(&path)).await {
        Ok(file) if file.starts_with(&root) => file,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mime = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match tokio::fs::read(file).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
