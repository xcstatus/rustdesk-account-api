use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    limits::LoginLimiter,
    oidc::OidcService,
    store::{DeviceLogin, SessionKind, Store, User},
};

#[derive(Clone)]
struct AppState {
    store: Store,
    session_ttl: i64,
    oidc_enabled: bool,
    login_limiter: LoginLimiter,
}

pub fn router(store: Store, session_ttl: i64, oidc: Option<Arc<OidcService>>) -> Router {
    router_with_limiter(store, session_ttl, oidc, LoginLimiter::default())
}

pub fn router_with_limiter(
    store: Store,
    session_ttl: i64,
    oidc: Option<Arc<OidcService>>,
    login_limiter: LoginLimiter,
) -> Router {
    let state = AppState {
        store,
        session_ttl,
        oidc_enabled: oidc.is_some(),
        login_limiter,
    };
    let mut app = Router::new()
        .route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/login", post(login))
        .route("/api/currentUser", post(current_user))
        .route("/api/logout", post(logout))
        .route("/api/login-options", get(login_options))
        .route("/api/ab/personal", post(personal_probe))
        .route("/api/ab", get(read_book).post(write_book))
        .route("/api/device-group/accessible", get(empty_list))
        .route("/api/users", get(directory_users))
        .route("/api/peers", get(directory_peers))
        .with_state(state);
    if let Some(oidc) = oidc {
        app = app.merge(oidc.router());
    }
    app.layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                response.headers_mut().insert(
                    header::REFERRER_POLICY,
                    header::HeaderValue::from_static("no-referrer"),
                );
                response.headers_mut().insert(
                    header::X_CONTENT_TYPE_OPTIONS,
                    header::HeaderValue::from_static("nosniff"),
                );
                response
            },
        ))
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
fn internal(_: anyhow::Error) -> Response {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Account storage unavailable",
    )
}
pub fn user_payload(user: &User) -> Value {
    json!({"id":user.id,"name":user.username,"display_name":user.display_name,"is_admin":user.role=="admin","status":1,"info":{}})
}
async fn authenticated(state: &AppState, headers: &HeaderMap) -> Result<(String, User), Response> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid or expired session"))?;
    let user = state
        .store
        .authenticate(token)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid or expired session"))?;
    Ok((token.to_owned(), user))
}
#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    uuid: String,
    #[serde(default, rename = "deviceInfo")]
    device_info: DeviceMetadata,
}
#[derive(Default, Deserialize)]
struct DeviceMetadata {
    #[serde(default)]
    os: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    device_type: String,
}
async fn login(State(state): State<AppState>, Json(input): Json<Login>) -> Response {
    if input.kind != "account" || input.username.len() > 256 || input.password.len() > 4096 {
        if let Err(e) = state
            .store
            .record_login_failure("local", Some(SessionKind::Client), "invalid_request")
            .await
        {
            return internal(e);
        }
        return error(StatusCode::BAD_REQUEST, "Invalid account request");
    }
    if !state.login_limiter.allow(&input.username).await {
        if let Err(e) = state
            .store
            .record_login_failure("local", Some(SessionKind::Client), "rate_limited")
            .await
        {
            return internal(e);
        }
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many login attempts");
    }
    match state
        .store
        .login_local_with_device(
            &input.username,
            &input.password,
            state.session_ttl,
            SessionKind::Client,
            Some(&DeviceLogin {
                rustdesk_id: input.id,
                uuid: input.uuid,
                platform: input.device_info.os,
                name: input.device_info.name,
                device_type: input.device_info.device_type,
            }),
        )
        .await
    {
        Ok(Some((user, token))) => {
            Json(json!({"access_token":token,"type":"access_token","user":user_payload(&user)}))
                .into_response()
        }
        Ok(None) => error(StatusCode::UNAUTHORIZED, "Invalid username or password"),
        Err(e) => internal(e),
    }
}
async fn current_user(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match authenticated(&state, &headers).await {
        Ok((_, user)) => Json(user_payload(&user)).into_response(),
        Err(e) => e,
    }
}
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (token, _) = match authenticated(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    match state
        .store
        .logout_session(&token, SessionKind::Client)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => internal(e),
    }
}
async fn login_options(State(state): State<AppState>) -> Json<Value> {
    Json(if state.oidc_enabled {
        json!(["oidc/company"])
    } else {
        json!([])
    })
}
async fn personal_probe(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = authenticated(&state, &headers).await {
        return e;
    }
    error(StatusCode::NOT_FOUND, "Legacy personal address book")
}
async fn empty_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = authenticated(&state, &headers).await {
        return e;
    }
    Json(json!({"total":0,"data":[]})).into_response()
}

#[derive(Deserialize)]
struct DirectoryPage {
    #[serde(default = "first_page")]
    current: usize,
    #[serde(default = "default_page_size", rename = "pageSize")]
    page_size: usize,
}
fn first_page() -> usize {
    1
}
fn default_page_size() -> usize {
    100
}
fn directory_page<T>(items: Vec<T>, page: &DirectoryPage) -> Result<Value, &'static str>
where
    T: serde::Serialize,
{
    if page.current == 0 || page.page_size == 0 || page.page_size > 1000 {
        return Err("Invalid directory pagination");
    }
    let offset = page
        .current
        .saturating_sub(1)
        .saturating_mul(page.page_size);
    let total = items.len();
    let data: Vec<T> = items
        .into_iter()
        .skip(offset)
        .take(page.page_size)
        .collect();
    Ok(json!({"total":total,"data":data}))
}
async fn directory_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<DirectoryPage>,
) -> Response {
    let (_, user) = match authenticated(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    match directory_page(vec![user_payload(&user)], &page) {
        Ok(value) => Json(value).into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, message),
    }
}
async fn directory_peers(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<DirectoryPage>,
) -> Response {
    let (token, user) = match authenticated(&state, &headers).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    let devices = match state
        .store
        .list_devices(&token, SessionKind::Client, false)
        .await
    {
        Ok(value) => value,
        Err(error) => return crate::web_auth::account_error(error),
    };
    let peers: Vec<Value> = devices
        .into_iter()
        .map(|device| {
            json!({
                "id":device.rustdesk_id,"status":1,"user":user.id,"user_name":user.username,
                "device_group_name":"","note":device.alias,
                "info":{"device_name":device.device_name,"os":device.platform,"username":""}
            })
        })
        .collect();
    match directory_page(peers, &page) {
        Ok(value) => Json(value).into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, message),
    }
}
async fn read_book(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (token, _) = match authenticated(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    match state.store.get_book_authenticated(&token).await {
        Ok(book) => Json(json!({"data":book.to_string(),"licensed_devices":0})).into_response(),
        Err(e) => crate::web_auth::account_error(e),
    }
}
#[derive(Deserialize)]
struct Snapshot {
    data: String,
}
async fn write_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Snapshot>,
) -> Response {
    let (token, _) = match authenticated(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let snapshot = match serde_json::from_str::<Value>(&input.data)
        .ok()
        .and_then(|v| address_book_snapshot(&v, state.store.credential_sync_enabled()))
    {
        Some(v) => v,
        None => return error(StatusCode::BAD_REQUEST, "Invalid address book snapshot"),
    };
    match state.store.put_book_authenticated(&token, &snapshot).await {
        Ok(()) => {
            let mut response = StatusCode::OK.into_response();
            response.headers_mut().insert(
                "x-rustdesk-credential-sync",
                header::HeaderValue::from_static(if state.store.credential_sync_enabled() {
                    "enabled; personal-salted-hash"
                } else {
                    "disabled; metadata-only"
                }),
            );
            response
        }
        Err(e) => crate::web_auth::account_error(e),
    }
}

// Personal hash is a credential; Store separates and encrypts it before persistence.
// Plaintext shared-book passwords and arbitrary fields are never accepted here.
fn address_book_snapshot(input: &Value, credentials_enabled: bool) -> Option<Value> {
    let object = input.as_object()?;
    let peers = object.get("peers")?.as_array()?;
    let tags = object.get("tags")?.as_array()?;
    if peers.len() > 1000
        || tags.len() > 1000
        || !tags
            .iter()
            .all(|v| v.as_str().is_some_and(|s| s.len() <= 256))
    {
        return None;
    }
    let mut sanitized = Vec::with_capacity(peers.len());
    let mut identifiers = std::collections::HashSet::with_capacity(peers.len());
    for peer in peers {
        let mut p = serde_json::Map::new();
        let id = peer.get("id")?.as_str()?;
        if id.is_empty() || id.len() > 256 || !identifiers.insert(id) {
            return None;
        }
        p.insert("id".into(), json!(id));
        if credentials_enabled && let Some(hash) = peer.get("hash") {
            let hash = hash.as_str()?;
            if !hash.is_empty()
                && crate::credentials::CredentialCipher::validate_hash(hash).is_err()
            {
                return None;
            }
            p.insert("hash".into(), json!(hash));
        }
        for key in ["alias", "platform", "username", "hostname"] {
            if let Some(v) = peer.get(key) {
                if v.as_str().is_none_or(|s| s.len() > 4096) {
                    return None;
                }
                p.insert(key.into(), v.clone());
            }
        }
        if let Some(v) = peer.get("tags") {
            if !v.as_array().is_some_and(|a| {
                a.len() <= 1000 && a.iter().all(|v| v.as_str().is_some_and(|s| s.len() <= 256))
            }) {
                return None;
            }
            p.insert("tags".into(), v.clone());
        }
        sanitized.push(Value::Object(p));
    }
    let mut result = json!({"peers":sanitized,"tags":tags});
    if let Some(colors) = object.get("tag_colors") {
        let colors = colors.as_str()?;
        let parsed = serde_json::from_str::<Value>(colors).ok()?;
        if !parsed
            .as_object()?
            .values()
            .all(|v| v.as_u64().is_some_and(|n| n <= u32::MAX as u64))
        {
            return None;
        }
        result["tag_colors"] = json!(colors);
    }
    Some(result)
}
