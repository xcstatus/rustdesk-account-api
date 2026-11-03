//! Same-origin browser authentication, separate from RustDesk bearer sessions.
use std::sync::Arc;

use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::{Host, Url};

use crate::{
    limits::LoginLimiter,
    oidc::OidcService,
    store::{AccountError, SessionKind, Store, User},
};

pub const SESSION_COOKIE: &str = "rd_web_session";
pub const OIDC_COOKIE: &str = "rd_web_oidc";

#[derive(Clone)]
pub struct WebAuth {
    pub store: Store,
    pub session_ttl: i64,
    pub origin: String,
    pub secure_cookies: bool,
    login_limiter: LoginLimiter,
}

impl WebAuth {
    pub fn new(store: Store, session_ttl: i64, origin: &str) -> Result<Self> {
        let url = Url::parse(origin)?;
        let local = match url.host() {
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            Some(Host::Domain(host)) => host == "localhost",
            _ => false,
        };
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "RD_WEB_ORIGIN must be an origin without credentials, path or query"
        );
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && local),
            "Web origin requires HTTPS, except local development"
        );
        ensure!(
            session_ttl > 0 && session_ttl <= 2_592_000,
            "Web session lifetime must be 1..2592000 seconds"
        );
        Ok(Self {
            store,
            session_ttl,
            origin: url.origin().ascii_serialization(),
            secure_cookies: url.scheme() == "https",
            login_limiter: LoginLimiter::default(),
        })
    }

    pub fn with_login_limiter(mut self, limiter: LoginLimiter) -> Self {
        self.login_limiter = limiter;
        self
    }

    pub fn check_origin(&self, headers: &HeaderMap) -> std::result::Result<(), &'static str> {
        if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(self.origin.as_str()) {
            return Err("Origin rejected");
        }
        Ok(())
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        csrf_required: bool,
    ) -> std::result::Result<(String, User), Response> {
        let token = cookie(headers, SESSION_COOKIE)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Sign in required"))?;
        let user = self
            .store
            .authenticate_web(&token)
            .await
            .map_err(account_error)?
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Sign in required"))?;
        if csrf_required {
            self.check_origin(headers)
                .map_err(|message| error(StatusCode::FORBIDDEN, message))?;
            let expected = csrf_token(&token);
            let actual = headers
                .get("x-csrf-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !same_secret(actual, &expected) {
                return Err(error(StatusCode::FORBIDDEN, "CSRF token rejected"));
            }
        }
        Ok((token, user))
    }

    pub fn session_cookie(&self, token: &str) -> String {
        format!(
            "{SESSION_COOKIE}={token}; Path=/admin/api/v1; HttpOnly; SameSite=Lax; Max-Age={}{}",
            self.session_ttl,
            self.secure_suffix()
        )
    }
    pub fn clear_session_cookie(&self) -> String {
        format!(
            "{SESSION_COOKIE}=; Path=/admin/api/v1; HttpOnly; SameSite=Lax; Max-Age=0{}",
            self.secure_suffix()
        )
    }
    pub fn oidc_cookie(&self, binding: &str) -> String {
        format!(
            "{OIDC_COOKIE}={binding}; Path=/api/oidc/callback; HttpOnly; SameSite=Lax; Max-Age=180{}",
            self.secure_suffix()
        )
    }
    pub fn clear_oidc_cookie(&self) -> String {
        format!(
            "{OIDC_COOKIE}=; Path=/api/oidc/callback; HttpOnly; SameSite=Lax; Max-Age=0{}",
            self.secure_suffix()
        )
    }
    fn secure_suffix(&self) -> &'static str {
        if self.secure_cookies { "; Secure" } else { "" }
    }
}

#[derive(Clone)]
struct AuthState {
    auth: WebAuth,
    oidc: Option<Arc<OidcService>>,
}

pub fn router(auth: WebAuth, oidc: Option<Arc<OidcService>>) -> Router {
    Router::new()
        .route("/admin/api/v1/auth/options", get(options))
        .route("/admin/api/v1/auth/login", post(login))
        .route("/admin/api/v1/auth/logout", post(logout))
        .route("/admin/api/v1/auth/oidc", post(oidc_start))
        .route("/admin/api/v1/me", get(me))
        .with_state(AuthState { auth, oidc })
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
        .layer(axum::middleware::map_response(security_headers))
}

pub async fn security_headers(mut response: Response) -> Response {
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        response.headers_mut().insert(name, value.parse().unwrap());
    }
    response.headers_mut().entry(header::CONTENT_SECURITY_POLICY).or_insert(header::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
}

pub fn csrf_token(token: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"rustdesk-web-csrf-v1:");
    hash.update(token.as_bytes());
    hex::encode(hash.finalize())
}

pub fn user_payload(user: &User) -> Value {
    json!({"id": user.id, "username": user.username, "display_name": user.display_name, "role": user.role, "source": user.source, "disabled": user.disabled})
}

pub fn account_error(error: anyhow::Error) -> Response {
    match error.downcast_ref::<AccountError>() {
        Some(AccountError::InvalidInput(message)) => self::error(StatusCode::BAD_REQUEST, message),
        Some(AccountError::Unauthorized) => {
            self::error(StatusCode::UNAUTHORIZED, "Sign in required")
        }
        Some(AccountError::Forbidden) => self::error(StatusCode::FORBIDDEN, "Permission denied"),
        Some(AccountError::NotFound) => {
            self::error(StatusCode::NOT_FOUND, "Account or session not found")
        }
        Some(AccountError::Conflict(message)) => self::error(StatusCode::CONFLICT, message),
        None => self::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Account storage unavailable",
        ),
    }
}

pub fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for line in headers.get_all(header::COOKIE) {
        for pair in line.to_str().ok()?.split(';') {
            let Some((key, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if key == name {
                if found.is_some()
                    || value.len() != 64
                    || !value.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return None;
                }
                found = Some(value.to_owned());
            }
        }
    }
    found
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.as_bytes()
            .iter()
            .zip(b.as_bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

async fn options(State(state): State<AuthState>) -> Json<Value> {
    Json(json!({"oidc_enabled": state.oidc.is_some()}))
}
#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
}
async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Response {
    if let Err(message) = state.auth.check_origin(&headers) {
        if let Err(e) = state
            .auth
            .store
            .record_login_failure("local", Some(SessionKind::Web), "invalid_origin")
            .await
        {
            return account_error(e);
        }
        return error(StatusCode::FORBIDDEN, message);
    }
    if input.username.len() > 128 || input.password.len() > 4096 {
        if let Err(e) = state
            .auth
            .store
            .record_login_failure("local", Some(SessionKind::Web), "invalid_request")
            .await
        {
            return account_error(e);
        }
        return error(StatusCode::BAD_REQUEST, "Invalid account request");
    }
    if !state.auth.login_limiter.allow(&input.username).await {
        if let Err(e) = state
            .auth
            .store
            .record_login_failure("local", Some(SessionKind::Web), "rate_limited")
            .await
        {
            return account_error(e);
        }
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many login attempts");
    }
    match state
        .auth
        .store
        .login_local(
            &input.username,
            &input.password,
            state.auth.session_ttl,
            SessionKind::Web,
        )
        .await
    {
        Ok(Some((user, token))) => {
            let mut response =
                Json(json!({"user": user_payload(&user), "csrf_token": csrf_token(&token)}))
                    .into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                state.auth.session_cookie(&token).parse().unwrap(),
            );
            response
        }
        Ok(None) => error(StatusCode::UNAUTHORIZED, "Invalid username or password"),
        Err(e) => account_error(e),
    }
}
async fn me(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    match state.auth.authenticate(&headers, false).await {
        Ok((token, user)) => {
            Json(json!({"user": user_payload(&user), "csrf_token": csrf_token(&token)}))
                .into_response()
        }
        Err(e) => e,
    }
}
async fn logout(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (token, _) = match state.auth.authenticate(&headers, true).await {
        Ok(value) => value,
        Err(e) => return e,
    };
    if let Err(e) = state
        .auth
        .store
        .logout_session(&token, SessionKind::Web)
        .await
    {
        return account_error(e);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        state.auth.clear_session_cookie().parse().unwrap(),
    );
    response
}
async fn oidc_start(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if let Err(message) = state.auth.check_origin(&headers) {
        return error(StatusCode::FORBIDDEN, message);
    }
    match state.oidc {
        Some(service) => service.start_web(state.auth).await,
        None => error(StatusCode::NOT_FOUND, "Unified sign in is not configured"),
    }
}
