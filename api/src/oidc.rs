//! Single-provider OIDC and the official RustDesk browser/polling adapter.
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use openidconnect::{
    AccessTokenHash, AdditionalClaims, AsyncHttpClient, AuthorizationCode, Client, ClientId,
    ClientSecret, CsrfToken, EmptyExtraTokenFields, EndpointMaybeSet, EndpointNotSet, EndpointSet,
    HttpRequest, HttpResponse, IdTokenFields, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, StandardErrorResponse,
    StandardTokenResponse, TokenResponse,
    core::{
        CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
        CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreRevocableToken,
        CoreRevocationErrorResponse, CoreTokenIntrospectionResponse, CoreTokenType,
    },
    reqwest,
};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::{Host, Url};

use crate::store::{SessionKind, Store, User};
use crate::web_auth::{self, WebAuth};

const ATTEMPT_TTL: Duration = Duration::from_secs(180);
const MAX_ATTEMPTS: usize = 1024;
const PENDING_ERROR: &str = "No authed oidc is found";

#[derive(Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub claim_path: Vec<String>,
    pub admin_values: Vec<String>,
    pub member_values: Vec<String>,
    pub unmatched_member: bool,
    pub allow_http_loopback: bool,
    pub session_ttl_seconds: i64,
}

// Extension fields are deserialized by openidconnect as part of its typed ID token.
// They are never read until IdToken::claims has validated the signed token.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct TrustedClaims {
    #[serde(flatten)]
    fields: serde_json::Map<String, Value>,
}
impl AdditionalClaims for TrustedClaims {}

type ProviderToken = StandardTokenResponse<
    IdTokenFields<
        TrustedClaims,
        EmptyExtraTokenFields,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >,
    CoreTokenType,
>;
type ProviderClient = Client<
    TrustedClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    ProviderToken,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

pub struct OidcService {
    config: OidcConfig,
    store: Store,
    client: ProviderClient,
    http: SecureHttpClient,
    attempts: Mutex<Attempts>,
}

// Deliberately no Debug: pending contexts contain nonce and PKCE secrets.
struct CallbackContext {
    nonce: Nonce,
    verifier: PkceCodeVerifier,
}
enum AttemptStatus {
    Waiting(Option<CallbackContext>),
    Ready(User),
    Failed,
}
struct Attempt {
    binding: [u8; 32],
    expires: Instant,
    state_key: [u8; 32],
    status: AttemptStatus,
    purpose: AttemptPurpose,
}
enum AttemptPurpose {
    Client,
    Web {
        browser_binding: [u8; 32],
        auth: WebAuth,
    },
}
#[derive(Default)]
struct Attempts {
    polls: HashMap<[u8; 32], Attempt>,
    states: HashMap<[u8; 32], [u8; 32]>,
}
impl Attempts {
    fn prune(&mut self, now: Instant) {
        self.polls.retain(|_, attempt| attempt.expires > now);
        self.states.retain(|_, poll| self.polls.contains_key(poll));
    }

    fn take_callback(
        &mut self,
        state: &str,
        browser: Option<&str>,
        now: Instant,
    ) -> Option<([u8; 32], CallbackContext, Option<WebAuth>)> {
        self.prune(now);
        let key = digest(state.as_bytes());
        let poll = *self.states.get(&key)?;
        let attempt = self.polls.get_mut(&poll)?;
        let auth = match &attempt.purpose {
            AttemptPurpose::Client => None,
            AttemptPurpose::Web {
                browser_binding,
                auth,
            } => {
                if digest(browser?.as_bytes()) != *browser_binding {
                    return None;
                }
                Some(auth.clone())
            }
        };
        self.states.remove(&key);
        match &mut attempt.status {
            AttemptStatus::Waiting(context) => context.take().map(|context| (poll, context, auth)),
            _ => None,
        }
    }

    fn poll(&mut self, code: &str, id: &str, uuid: &str, now: Instant) -> PollResult {
        self.prune(now);
        let key = digest(code.as_bytes());
        let Some(attempt) = self.polls.get(&key) else {
            return PollResult::Invalid;
        };
        if !matches!(attempt.purpose, AttemptPurpose::Client)
            || attempt.binding != binding(id, uuid)
        {
            return PollResult::Invalid;
        }
        if matches!(attempt.status, AttemptStatus::Waiting(_)) {
            return PollResult::Pending;
        }
        let Some(attempt) = self.polls.remove(&key) else {
            return PollResult::Invalid;
        };
        self.states.remove(&attempt.state_key);
        match attempt.status {
            AttemptStatus::Ready(user) => PollResult::Ready(user),
            AttemptStatus::Failed => PollResult::Failed,
            _ => PollResult::Invalid,
        }
    }
}
enum PollResult {
    Pending,
    Ready(User),
    Failed,
    Invalid,
}

// Validate *every* outgoing URL, including the JWKS URL fetched during discovery.
// Redirect following is disabled, so provider metadata cannot downgrade transport.
#[derive(Clone)]
struct SecureHttpClient {
    client: reqwest::Client,
    allow_http_loopback: bool,
}
#[derive(Debug)]
struct HttpFailure;
#[derive(Debug)]
struct AuditedOidcRejection;
impl std::fmt::Display for AuditedOidcRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OIDC identity not admitted")
    }
}
impl std::error::Error for AuditedOidcRejection {}
impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OIDC HTTP request failed")
    }
}
impl std::error::Error for HttpFailure {}
impl<'c> AsyncHttpClient<'c> for SecureHttpClient {
    type Error = HttpFailure;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<HttpResponse, HttpFailure>> + Send + 'c>>;
    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            validate_transport(&request.uri().to_string(), self.allow_http_loopback)
                .map_err(|_| HttpFailure)?;
            self.client.call(request).await.map_err(|_| HttpFailure)
        })
    }
}

impl OidcService {
    /// Browser attempts cannot be collected through the RustDesk polling adapter.
    pub async fn start_web(&self, auth: WebAuth) -> Response {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = self
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("profile".into()))
            .set_pkce_challenge(challenge)
            .url();
        let secret = random_secret();
        let poll_key = digest(random_secret().as_bytes());
        let mut attempts = self.attempts.lock().await;
        let now = Instant::now();
        attempts.prune(now);
        if attempts.polls.len() >= MAX_ATTEMPTS {
            return error(StatusCode::TOO_MANY_REQUESTS, "Too many OIDC requests");
        }
        let state_key = digest(state.secret().as_bytes());
        attempts.states.insert(state_key, poll_key);
        attempts.polls.insert(
            poll_key,
            Attempt {
                binding: [0; 32],
                expires: now + ATTEMPT_TTL,
                state_key,
                status: AttemptStatus::Waiting(Some(CallbackContext { nonce, verifier })),
                purpose: AttemptPurpose::Web {
                    browser_binding: digest(secret.as_bytes()),
                    auth: auth.clone(),
                },
            },
        );
        let mut response = Json(json!({"url": url.as_str()})).into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            auth.oidc_cookie(&secret).parse().unwrap(),
        );
        response
    }

    pub async fn new(config: OidcConfig, store: Store) -> Result<Self> {
        validate_config(&config)?;
        let http = SecureHttpClient {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .context("build OIDC HTTP client")?,
            allow_http_loopback: config.allow_http_loopback,
        };
        let metadata =
            CoreProviderMetadata::discover_async(IssuerUrl::new(config.issuer.clone())?, &http)
                .await
                .map_err(|_| anyhow!("OIDC discovery failed"))?;
        validate_transport(
            metadata.authorization_endpoint().as_str(),
            config.allow_http_loopback,
        )?;
        validate_transport(metadata.jwks_uri().as_str(), config.allow_http_loopback)?;
        let token_endpoint = metadata
            .token_endpoint()
            .context("OIDC provider has no token endpoint")?;
        validate_transport(token_endpoint.as_str(), config.allow_http_loopback)?;
        let client = ProviderClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id.clone()),
            config.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(RedirectUrl::new(config.redirect_uri.clone())?);
        store.sync_policy_version(&policy_version(&config)).await?;
        Ok(Self {
            config,
            store,
            client,
            http,
            attempts: Mutex::new(Attempts::default()),
        })
    }

    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/api/oidc/auth", post(start))
            .route("/api/oidc/auth-query", get(poll))
            .route("/api/oidc/callback", get(callback))
            .with_state(self)
            .layer(middleware::map_response(
                |mut response: Response| async move {
                    response.headers_mut().insert(
                        header::CACHE_CONTROL,
                        header::HeaderValue::from_static("no-store"),
                    );
                    response
                        .headers_mut()
                        .insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
                    response.headers_mut().insert(
                        header::REFERRER_POLICY,
                        header::HeaderValue::from_static("no-referrer"),
                    );
                    response
                },
            ))
    }

    async fn audit_failure(&self, kind: Option<SessionKind>, reason: &str) -> bool {
        match self.store.record_login_failure("oidc", kind, reason).await {
            Ok(()) => true,
            Err(_) => {
                tracing::error!("OIDC failure audit storage unavailable");
                false
            }
        }
    }

    async fn complete(&self, code: String, context: CallbackContext) -> Result<User> {
        let response = self
            .client
            .exchange_code(AuthorizationCode::new(code))?
            .set_pkce_verifier(context.verifier)
            .request_async(&self.http)
            .await
            .map_err(|_| anyhow!("OIDC code exchange failed"))?;
        let id_token = response.id_token().context("OIDC ID token missing")?;
        let verifier = self.client.id_token_verifier();
        // Library verifies signature, issuer, audience, expiry and nonce. Never disable any check.
        let claims = id_token
            .claims(&verifier, &context.nonce)
            .map_err(|_| anyhow!("OIDC ID token rejected"))?;
        if let Some(expected) = claims.access_token_hash() {
            let actual = AccessTokenHash::from_token(
                response.access_token(),
                id_token.signing_alg()?,
                id_token.signing_key(&verifier)?,
            )?;
            ensure!(actual == *expected, "OIDC access token hash rejected");
        }
        ensure!(
            !claims.subject().as_str().is_empty(),
            "OIDC subject missing"
        );
        // OIDC azp is required when audiences are multiple, and must identify this RP.
        if claims.audiences().len() > 1 {
            ensure!(
                claims.authorized_party().is_some(),
                "OIDC authorized party missing"
            );
        }
        if let Some(azp) = claims.authorized_party() {
            ensure!(
                azp.as_str() == self.config.client_id,
                "OIDC authorized party rejected"
            );
        }
        let role = mapped_role(&self.config, &claims.additional_claims().fields);
        let display_name = claims
            .name()
            .and_then(|name| name.get(None))
            .map(|name| name.as_str())
            .or_else(|| claims.preferred_username().map(|name| name.as_str()))
            .unwrap_or_else(|| claims.subject().as_str());
        // A malformed role claim also removes admission for a known, verified identity.
        let user = self
            .store
            .find_or_create_oidc(
                claims.issuer().as_str(),
                claims.subject().as_str(),
                display_name,
                role.as_ref().ok().copied().flatten(),
            )
            .await?;
        if role.is_err() || user.is_none() {
            return Err(AuditedOidcRejection.into());
        }
        user.context("OIDC identity not admitted")
    }
}

#[derive(Deserialize)]
struct StartRequest {
    op: String,
    id: String,
    uuid: String,
}
#[derive(Deserialize)]
struct PollRequest {
    code: String,
    id: String,
    uuid: String,
}
#[derive(Deserialize)]
struct CallbackRequest {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

async fn start(
    State(service): State<Arc<OidcService>>,
    Json(request): Json<StartRequest>,
) -> Response {
    if request.op != "company" || !valid_binding(&request.id, &request.uuid) {
        if !service
            .audit_failure(Some(SessionKind::Client), "invalid_request")
            .await
        {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Account storage unavailable",
            );
        }
        return error(StatusCode::BAD_REQUEST, "Invalid OIDC request");
    }
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state, nonce) = service
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(challenge)
        .url();
    let code = random_secret();
    let mut attempts = service.attempts.lock().await;
    let now = Instant::now();
    attempts.prune(now);
    if attempts.polls.len() >= MAX_ATTEMPTS {
        return error(StatusCode::TOO_MANY_REQUESTS, "Too many OIDC requests");
    }
    let state_key = digest(state.secret().as_bytes());
    let poll_key = digest(code.as_bytes());
    attempts.states.insert(state_key, poll_key);
    attempts.polls.insert(
        poll_key,
        Attempt {
            binding: binding(&request.id, &request.uuid),
            expires: now + ATTEMPT_TTL,
            state_key,
            status: AttemptStatus::Waiting(Some(CallbackContext { nonce, verifier })),
            purpose: AttemptPurpose::Client,
        },
    );
    Json(json!({"code": code, "url": url.as_str()})).into_response()
}

async fn poll(
    State(service): State<Arc<OidcService>>,
    Query(request): Query<PollRequest>,
) -> Response {
    if !valid_binding(&request.id, &request.uuid) || request.code.len() > 256 {
        return error(StatusCode::BAD_REQUEST, "Invalid or expired OIDC attempt");
    }
    let result = service.attempts.lock().await.poll(
        &request.code,
        &request.id,
        &request.uuid,
        Instant::now(),
    );
    match result {
        PollResult::Pending => Json(json!({"error": PENDING_ERROR})).into_response(),
        // A failed callback has already persisted its failure event. Polling/replaying
        // a collection credential is not a new authentication attempt.
        PollResult::Failed | PollResult::Invalid => {
            error(StatusCode::BAD_REQUEST, "Invalid or expired OIDC attempt")
        }
        PollResult::Ready(user) => {
            let token = match service
                .store
                .issue_session_with_device(
                    &user.id,
                    service.config.session_ttl_seconds,
                    Some(&crate::store::DeviceLogin {
                        rustdesk_id: request.id,
                        uuid: request.uuid,
                        platform: String::new(),
                        name: String::new(),
                        device_type: "client".into(),
                    }),
                )
                .await
            {
                Ok(token) => token,
                Err(_) => return error(StatusCode::UNAUTHORIZED, "OIDC authentication failed"),
            };
            // Recheck current disabled/admission/role state at collection, rather than using the
            // stale user snapshot from the browser callback.
            match service.store.authenticate(&token).await {
                Ok(Some(user)) => Json(
                    json!({"access_token": token, "type": "access_token", "user": {
                        "id": user.id, "name": user.username, "display_name": user.display_name,
                        "is_admin": user.role == "admin", "status": 1, "info": {},
                    }}),
                )
                .into_response(),
                _ => {
                    let _ = service.store.revoke_session(&token).await;
                    error(StatusCode::UNAUTHORIZED, "OIDC authentication failed")
                }
            }
        }
    }
}

async fn callback(
    State(service): State<Arc<OidcService>>,
    headers: HeaderMap,
    Query(request): Query<CallbackRequest>,
) -> Response {
    let Some(state) = request.state.filter(|state| state.len() <= 256) else {
        if !service.audit_failure(None, "oidc_state").await {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Account storage unavailable",
            );
        }
        return callback_page(false);
    };
    // Consumed before any code exchange, including provider errors and malformed callbacks.
    let taken = service.attempts.lock().await.take_callback(
        &state,
        web_auth::cookie(&headers, web_auth::OIDC_COOKIE).as_deref(),
        Instant::now(),
    );
    let Some((poll_key, context, web)) = taken else {
        if !service.audit_failure(None, "oidc_state").await {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Account storage unavailable",
            );
        }
        return callback_page(false);
    };
    let kind = Some(if web.is_some() {
        SessionKind::Web
    } else {
        SessionKind::Client
    });
    let provider_failure = request.error.is_some()
        || request
            .code
            .as_ref()
            .is_none_or(|c| c.is_empty() || c.len() > 8192);
    let result = match (request.error, request.code) {
        (None, Some(code)) if !code.is_empty() && code.len() <= 8192 => {
            service.complete(code, context).await
        }
        _ => Err(anyhow!("OIDC callback rejected")),
    };
    let authentication_failed = result.is_err();
    let already_audited = result
        .as_ref()
        .err()
        .is_some_and(|e| e.is::<AuditedOidcRejection>());
    if authentication_failed
        && !already_audited
        && !service
            .audit_failure(
                kind,
                if provider_failure {
                    "oidc_provider"
                } else {
                    "oidc_verification"
                },
            )
            .await
    {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Account storage unavailable",
        );
    }
    let mut attempts = service.attempts.lock().await;
    attempts.prune(Instant::now());
    if let Some(auth) = web {
        let live = attempts.polls.remove(&poll_key).is_some();
        drop(attempts);
        let token = if live {
            match result {
                Ok(user) => service
                    .store
                    .issue_web_session(&user.id, auth.session_ttl)
                    .await
                    .ok(),
                Err(_) => None,
            }
        } else {
            None
        };
        if token.is_none()
            && !live
            && !authentication_failed
            && !service
                .audit_failure(Some(SessionKind::Web), "oidc_expired")
                .await
        {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Account storage unavailable",
            );
        }
        let target = format!(
            "{}/admin/{}",
            auth.origin,
            if token.is_some() { "" } else { "?login=failed" }
        );
        let mut response = Redirect::to(&target).into_response();
        response.headers_mut().append(
            header::SET_COOKIE,
            auth.clear_oidc_cookie().parse().unwrap(),
        );
        if let Some(token) = token {
            response.headers_mut().append(
                header::SET_COOKIE,
                auth.session_cookie(&token).parse().unwrap(),
            );
        }
        return response;
    }
    let Some(attempt) = attempts.polls.get_mut(&poll_key) else {
        return callback_page(false);
    };
    let success = result.is_ok();
    attempt.status = match result {
        Ok(user) => AttemptStatus::Ready(user),
        Err(_) => AttemptStatus::Failed,
    };
    callback_page(success)
}

fn callback_page(success: bool) -> Response {
    let (status, text) = if success {
        (
            StatusCode::OK,
            "Sign-in completed. Return to RustDesk. You may close this window.",
        )
    } else {
        (
            StatusCode::BAD_REQUEST,
            "Sign-in failed or expired. Return to RustDesk and try again.",
        )
    };
    let mut response = (status, Html(format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>RustDesk sign-in</title><p>{text}</p></html>"))).into_response();
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static(
            "default-src 'none'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response
}
fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}
fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn binding(id: &str, uuid: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update((id.len() as u64).to_be_bytes());
    hash.update(id.as_bytes());
    hash.update(uuid.as_bytes());
    hash.finalize().into()
}
fn valid_binding(id: &str, uuid: &str) -> bool {
    !id.is_empty() && !uuid.is_empty() && id.len() <= 512 && uuid.len() <= 512
}

fn validate_transport(raw: &str, allow_http_loopback: bool) -> Result<()> {
    let url = Url::parse(raw).context("Invalid OIDC URL")?;
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "OIDC URL must not contain credentials or fragments"
    );
    let loopback = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(host)) => host == "localhost",
        None => false,
    };
    ensure!(
        url.scheme() == "https" || (allow_http_loopback && url.scheme() == "http" && loopback),
        "OIDC URL requires HTTPS (explicit development flag permits loopback HTTP)"
    );
    Ok(())
}
fn validate_config(config: &OidcConfig) -> Result<()> {
    validate_transport(&config.issuer, config.allow_http_loopback)?;
    validate_transport(&config.redirect_uri, config.allow_http_loopback)?;
    ensure!(
        Url::parse(&config.issuer)?.query().is_none(),
        "OIDC issuer must not contain query"
    );
    ensure!(!config.client_id.is_empty(), "OIDC client ID required");
    ensure!(
        config.session_ttl_seconds > 0,
        "OIDC session TTL must be positive"
    );
    ensure!(
        !config.claim_path.is_empty() && config.claim_path.iter().all(|part| !part.is_empty()),
        "OIDC role claim path required"
    );
    ensure!(
        config
            .admin_values
            .iter()
            .chain(&config.member_values)
            .all(|value| !value.is_empty()),
        "OIDC role values must be nonempty"
    );
    Ok(())
}
fn policy_version(config: &OidcConfig) -> String {
    let mut admin = config.admin_values.clone();
    admin.sort();
    admin.dedup();
    let mut member = config.member_values.clone();
    member.sort();
    member.dedup();
    let policy = json!({"v": 1, "issuer": config.issuer, "client_id": config.client_id,
        "redirect_uri": config.redirect_uri, "allow_http_loopback": config.allow_http_loopback,
        "session_ttl_seconds": config.session_ttl_seconds,
        "claim_path": config.claim_path, "admin": admin, "member": member, "fallback": config.unmatched_member});
    hex::encode(digest(policy.to_string().as_bytes()))
}
fn mapped_role(
    config: &OidcConfig,
    claims: &serde_json::Map<String, Value>,
) -> Result<Option<&'static str>> {
    let fallback = || {
        if config.unmatched_member {
            Some("member")
        } else {
            None
        }
    };
    let Some((first, rest)) = config.claim_path.split_first() else {
        bail!("OIDC role claim path missing");
    };
    let Some(mut value) = claims.get(first) else {
        return Ok(fallback());
    };
    for part in rest {
        let object = value
            .as_object()
            .context("Malformed OIDC role claim path")?;
        let Some(next) = object.get(part) else {
            return Ok(fallback());
        };
        value = next;
    }
    let values: Vec<&str> = match value {
        Value::String(value) => vec![value],
        Value::Array(values) => values
            .iter()
            .map(|value| value.as_str().context("Malformed OIDC role claim value"))
            .collect::<Result<_>>()?,
        _ => bail!("Malformed OIDC role claim value"),
    };
    if values
        .iter()
        .any(|value| config.admin_values.iter().any(|expected| expected == value))
    {
        return Ok(Some("admin"));
    }
    if values.iter().any(|value| {
        config
            .member_values
            .iter()
            .any(|expected| expected == value)
    }) {
        return Ok(Some("member"));
    }
    Ok(fallback())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> OidcConfig {
        OidcConfig {
            issuer: "https://id.example".into(),
            client_id: "rustdesk".into(),
            client_secret: None,
            redirect_uri: "https://api.example/api/oidc/callback".into(),
            claim_path: vec!["realm_access".into(), "roles".into()],
            admin_values: vec!["rd-admin".into()],
            member_values: vec!["rd-member".into()],
            unmatched_member: false,
            allow_http_loopback: false,
            session_ttl_seconds: 300,
        }
    }
    fn map(config: &OidcConfig, claims: Value) -> Result<Option<&'static str>> {
        mapped_role(config, claims.as_object().unwrap())
    }
    #[test]
    fn role_mapping_exact_admin_wins_and_default_deny() {
        let c = config();
        assert_eq!(
            map(
                &c,
                json!({"realm_access":{"roles":["rd-member", "rd-admin"]}})
            )
            .unwrap(),
            Some("admin")
        );
        assert_eq!(
            map(&c, json!({"realm_access":{"roles":"rd-member"}})).unwrap(),
            Some("member")
        );
        assert_eq!(
            map(
                &c,
                json!({"realm_access":{"roles":["RD-ADMIN", "rd-admin-other"]}})
            )
            .unwrap(),
            None
        );
        assert_eq!(map(&c, json!({})).unwrap(), None);
    }
    #[test]
    fn malformed_claim_never_uses_member_fallback() {
        let mut c = config();
        c.unmatched_member = true;
        assert_eq!(map(&c, json!({})).unwrap(), Some("member"));
        for claims in [
            json!({"realm_access":null}),
            json!({"realm_access":{"roles":42}}),
            json!({"realm_access":{"roles":["rd-admin", false]}}),
        ] {
            assert!(map(&c, claims).is_err());
        }
    }
    #[test]
    fn typed_id_token_extension_keeps_nested_claims() {
        let claims: openidconnect::IdTokenClaims<TrustedClaims, CoreGenderClaim> =
            serde_json::from_value(json!({
                "iss": "https://id.example", "sub": "subject", "aud": "rustdesk",
                "iat": 1700000000, "exp": 1700000300,
                "realm_access": {"roles": ["rd-member", "rd-admin"]},
            }))
            .unwrap();
        assert_eq!(
            mapped_role(&config(), &claims.additional_claims().fields).unwrap(),
            Some("admin")
        );
    }
    fn attempt(state: &str, status: AttemptStatus, now: Instant) -> Attempt {
        Attempt {
            binding: binding("id", "uuid"),
            expires: now + ATTEMPT_TTL,
            state_key: digest(state.as_bytes()),
            status,
            purpose: AttemptPurpose::Client,
        }
    }
    fn user() -> User {
        User {
            id: "user".into(),
            username: "name".into(),
            display_name: "Name".into(),
            role: "member".into(),
            source: "oidc".into(),
            disabled: false,
        }
    }
    #[test]
    fn polling_binding_expiry_and_single_collection() {
        let now = Instant::now();
        let mut attempts = Attempts::default();
        let key = digest(b"poll");
        attempts
            .polls
            .insert(key, attempt("state", AttemptStatus::Ready(user()), now));
        assert!(matches!(
            attempts.poll("poll", "other", "uuid", now),
            PollResult::Invalid
        ));
        assert!(matches!(
            attempts.poll("poll", "id", "other", now),
            PollResult::Invalid
        ));
        assert!(matches!(
            attempts.poll("poll", "id", "uuid", now),
            PollResult::Ready(_)
        ));
        assert!(matches!(
            attempts.poll("poll", "id", "uuid", now),
            PollResult::Invalid
        ));
        attempts
            .polls
            .insert(key, attempt("state", AttemptStatus::Ready(user()), now));
        assert!(matches!(
            attempts.poll("poll", "id", "uuid", now + ATTEMPT_TTL),
            PollResult::Invalid
        ));
    }
    #[test]
    fn completed_failure_is_collected_without_becoming_an_expiry_failure() {
        let now = Instant::now();
        let mut attempts = Attempts::default();
        attempts.polls.insert(
            digest(b"poll"),
            attempt("state", AttemptStatus::Failed, now),
        );
        assert!(matches!(
            attempts.poll("poll", "id", "uuid", now),
            PollResult::Failed
        ));
        assert!(matches!(
            attempts.poll("poll", "id", "uuid", now),
            PollResult::Invalid
        ));
    }
    #[test]
    fn callback_context_is_single_use_and_unrelated_state_cannot_consume() {
        let now = Instant::now();
        let mut attempts = Attempts::default();
        let key = digest(b"poll");
        let (_, verifier) = PkceCodeChallenge::new_random_sha256();
        attempts.polls.insert(
            key,
            attempt(
                "state",
                AttemptStatus::Waiting(Some(CallbackContext {
                    nonce: Nonce::new_random(),
                    verifier,
                })),
                now,
            ),
        );
        attempts.states.insert(digest(b"state"), key);
        assert!(attempts.take_callback("wrong", None, now).is_none());
        assert!(attempts.take_callback("state", None, now).is_some());
        assert!(attempts.take_callback("state", None, now).is_none());
        assert!(matches!(
            attempts.poll("poll", "id", "uuid", now),
            PollResult::Pending
        ));
    }
    #[tokio::test]
    async fn web_callback_requires_browser_binding_and_cannot_be_polled() {
        let now = Instant::now();
        let mut attempts = Attempts::default();
        let key = digest(b"poll");
        let (_, verifier) = PkceCodeChallenge::new_random_sha256();
        let auth = WebAuth::new(
            Store::connect("sqlite::memory:").await.unwrap(),
            3600,
            "https://api.example",
        )
        .unwrap();
        let mut entry = attempt(
            "state",
            AttemptStatus::Waiting(Some(CallbackContext {
                nonce: Nonce::new_random(),
                verifier,
            })),
            now,
        );
        entry.purpose = AttemptPurpose::Web {
            browser_binding: digest(b"browser"),
            auth,
        };
        attempts.polls.insert(key, entry);
        attempts.states.insert(digest(b"state"), key);
        assert!(matches!(
            attempts.poll("poll", "id", "uuid", now),
            PollResult::Invalid
        ));
        assert!(attempts.take_callback("state", None, now).is_none());
        assert!(
            attempts
                .take_callback("state", Some("wrong"), now)
                .is_none()
        );
        assert!(
            attempts
                .take_callback("state", Some("browser"), now)
                .unwrap()
                .2
                .is_some()
        );
        assert!(
            attempts
                .take_callback("state", Some("browser"), now)
                .is_none()
        );
    }
    #[test]
    fn transport_only_allows_explicit_loopback_http() {
        for url in [
            "http://localhost:8000",
            "http://127.0.0.1:8000",
            "http://[::1]:8000",
        ] {
            assert!(validate_transport(url, true).is_ok());
            assert!(validate_transport(url, false).is_err());
        }
        for url in [
            "http://id.example",
            "http://localhost.example",
            "https://user:pass@id.example",
            "https://id.example/#fragment",
        ] {
            assert!(validate_transport(url, true).is_err());
        }
        assert!(validate_transport("https://id.example", false).is_ok());
    }
    #[test]
    fn policy_version_changes_on_admission_change_not_order() {
        let mut c = config();
        let version = policy_version(&c);
        c.unmatched_member = true;
        assert_ne!(version, policy_version(&c));
        c.member_values = vec!["a".into(), "b".into()];
        let version = policy_version(&c);
        c.member_values.reverse();
        assert_eq!(version, policy_version(&c));
    }
}
