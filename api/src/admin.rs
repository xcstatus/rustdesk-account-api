//! Browser account management. Authorization is checked again by Store writes.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    store::{AuditFilter, SessionKind, User},
    web_auth::{WebAuth, account_error, user_payload},
};

pub fn router(auth: WebAuth) -> Router {
    Router::new()
        .route("/admin/api/v1/audit-events", get(list_audit_events))
        .route("/admin/api/v1/devices", get(list_devices))
        .route(
            "/admin/api/v1/devices/{id}",
            axum::routing::patch(assign_device),
        )
        .route("/admin/api/v1/users", get(list_users).post(create_user))
        .route(
            "/admin/api/v1/users/{id}",
            axum::routing::patch(update_user),
        )
        .route("/admin/api/v1/users/{id}/password", post(reset_password))
        .route("/admin/api/v1/me/password", post(change_password))
        .route(
            "/admin/api/v1/sessions",
            get(list_sessions).delete(revoke_sessions),
        )
        .route("/admin/api/v1/sessions/{id}", delete(revoke_session))
        .route(
            "/admin/api/v1/users/{id}/sessions",
            get(list_user_sessions).delete(revoke_user_sessions),
        )
        .with_state(auth)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}

async fn list_devices(State(auth): State<WebAuth>, headers: HeaderMap) -> Response {
    let (token, user) = match auth.authenticate(&headers, false).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    match auth
        .store
        .list_devices(&token, SessionKind::Web, user.role == "admin")
        .await
    {
        Ok(devices) => Json(json!({"devices":devices})).into_response(),
        Err(error) => account_error(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAssignment {
    owner_user_id: Value,
}
async fn assign_device(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(input): Json<DeviceAssignment>,
) -> Response {
    let token = match admin_session(&auth, &headers, true).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    let owner = match &input.owner_user_id {
        Value::Null => None,
        Value::String(value) if !value.is_empty() && value.len() <= 256 => Some(value.as_str()),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"Invalid device owner"})),
            )
                .into_response();
        }
    };
    if let Err(error) = auth.store.assign_device(&token, &device_id, owner).await {
        return account_error(error);
    }
    match auth
        .store
        .list_devices(&token, SessionKind::Web, true)
        .await
    {
        Ok(devices) => match devices.into_iter().find(|device| device.id == device_id) {
            Some(device) => Json(json!({"device":device})).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        Err(error) => account_error(error),
    }
}

async fn list_audit_events(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Query(filter): Query<AuditFilter>,
) -> Response {
    let token = match admin_session(&auth, &headers, false).await {
        Ok(token) => token,
        Err(error) => return error,
    };
    match auth.store.list_audit_events(&token, &filter).await {
        Ok(page) => Json(page).into_response(),
        Err(error) => account_error(error),
    }
}

fn require_admin(user: &User) -> Result<(), (StatusCode, Json<Value>)> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Administrator required"})),
        ))
    }
}

async fn admin_session(
    auth: &WebAuth,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<String, Response> {
    let (token, user) = auth.authenticate(headers, mutation).await?;
    require_admin(&user).map_err(IntoResponse::into_response)?;
    Ok(token)
}

async fn list_users(State(auth): State<WebAuth>, headers: HeaderMap) -> Response {
    let token = match admin_session(&auth, &headers, false).await {
        Ok(token) => token,
        Err(error) => return error,
    };
    match auth.store.list_users(&token).await {
        Ok(users) => Json(json!({"users":users})).into_response(),
        Err(error) => account_error(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUser {
    username: String,
    password: String,
    role: String,
}

async fn create_user(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Json(input): Json<CreateUser>,
) -> Response {
    let token = match admin_session(&auth, &headers, true).await {
        Ok(token) => token,
        Err(error) => return error,
    };
    match auth
        .store
        .admin_create_local(&token, &input.username, &input.password, &input.role)
        .await
    {
        Ok(user) => (
            StatusCode::CREATED,
            Json(json!({"user":user_payload(&user)})),
        )
            .into_response(),
        Err(error) => account_error(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUser {
    disabled: Option<bool>,
    role: Option<String>,
}

async fn update_user(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateUser>,
) -> Response {
    let token = match admin_session(&auth, &headers, true).await {
        Ok(token) => token,
        Err(error) => return error,
    };
    match auth
        .store
        .admin_update_user(&token, &id, input.disabled, input.role.as_deref())
        .await
    {
        Ok(user) => Json(json!({"user":user_payload(&user)})).into_response(),
        Err(error) => account_error(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetPassword {
    new_password: String,
}

async fn reset_password(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ResetPassword>,
) -> Response {
    let token = match admin_session(&auth, &headers, true).await {
        Ok(token) => token,
        Err(error) => return error,
    };
    match auth
        .store
        .admin_reset_password(&token, &id, &input.new_password)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangePassword {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Json(input): Json<ChangePassword>,
) -> Response {
    let (token, _) = match auth.authenticate(&headers, true).await {
        Ok(session) => session,
        Err(error) => return error,
    };
    match auth
        .store
        .change_password(&token, &input.current_password, &input.new_password)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

async fn list_sessions(State(auth): State<WebAuth>, headers: HeaderMap) -> Response {
    let (token, _) = match auth.authenticate(&headers, false).await {
        Ok(session) => session,
        Err(error) => return error,
    };
    match auth.store.list_sessions(&token, None).await {
        Ok(sessions) => Json(json!({"sessions":sessions})).into_response(),
        Err(error) => account_error(error),
    }
}

async fn list_user_sessions(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let token = match admin_session(&auth, &headers, false).await {
        Ok(token) => token,
        Err(error) => return error,
    };
    match auth.store.list_sessions(&token, Some(&id)).await {
        Ok(sessions) => Json(json!({"sessions":sessions})).into_response(),
        Err(error) => account_error(error),
    }
}

async fn revoke_session(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let (token, _) = match auth.authenticate(&headers, true).await {
        Ok(session) => session,
        Err(error) => return error,
    };
    match auth.store.revoke_managed_session(&token, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

async fn revoke_sessions(State(auth): State<WebAuth>, headers: HeaderMap) -> Response {
    let (token, _) = match auth.authenticate(&headers, true).await {
        Ok(session) => session,
        Err(error) => return error,
    };
    match auth.store.revoke_user_sessions(&token, None).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}

async fn revoke_user_sessions(
    State(auth): State<WebAuth>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let token = match admin_session(&auth, &headers, true).await {
        Ok(token) => token,
        Err(error) => return error,
    };
    match auth.store.revoke_user_sessions(&token, Some(&id)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => account_error(error),
    }
}
