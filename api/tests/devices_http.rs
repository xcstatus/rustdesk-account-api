use std::collections::BTreeSet;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use http_body_util::BodyExt;
use rustdesk_account_api::{
    admin, compat,
    store::Store,
    web_auth::{WebAuth, csrf_token},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const PASSWORD: &str = "device-http-password";
const ORIGIN: &str = "http://127.0.0.1:21114";

async fn request(
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
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    Ok((status, headers, body))
}

async fn client(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> Result<(StatusCode, HeaderMap, Value)> {
    request(
        app,
        method,
        path,
        &[("authorization", format!("Bearer {token}"))],
        body,
    )
    .await
}

async fn web(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> Result<(StatusCode, HeaderMap, Value)> {
    request(
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

async fn login(app: &Router, username: &str, device: &str) -> Result<String> {
    let (status, _, body) = request(
        app,
        "POST",
        "/api/login",
        &[],
        json!({
            "username":username,"password":PASSWORD,"type":"account","id":device,
            "uuid":format!("private-uuid-{device}"),
            "deviceInfo":{"os":"Linux","name":format!("Name {device}"),"type":"desktop"}
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    Ok(body["access_token"]
        .as_str()
        .context("client token missing")?
        .to_owned())
}

fn ids(items: &Value, key: &str) -> BTreeSet<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item[key].as_str().unwrap().to_owned())
        .collect()
}

fn assert_private(value: &Value) {
    let serialized = value.to_string();
    assert!(!serialized.contains("uuid"));
    assert!(!serialized.contains("private-uuid-"));
}

#[tokio::test]
async fn client_directories_paginate_owned_devices_and_current_user_only() -> Result<()> {
    let store = Store::connect("sqlite::memory:").await?;
    let alice = store.create_local("alice", PASSWORD, "admin").await?;
    let bob = store.create_local("bob", PASSWORD, "member").await?;
    let app = compat::router(store.clone(), 3600, None);
    let alice_token = login(&app, "alice", "alice-a").await?;
    login(&app, "alice", "alice-b").await?;
    login(&app, "alice", "alice-c").await?;
    login(&app, "alice", "alice-a").await?;
    let bob_token = login(&app, "bob", "bob-a").await?;

    // A personal saved peer is independent from the directory of owned devices.
    assert_eq!(
        client(
            &app,
            &alice_token,
            "POST",
            "/api/ab",
            json!({"data":json!({
        "peers":[{"id":"saved-remote-peer"}],"tags":[]
    }).to_string()})
        )
        .await?
        .0,
        StatusCode::OK
    );
    let (status, headers, all) =
        client(&app, &alice_token, "GET", "/api/peers", Value::Null).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(all["total"], 3);
    assert_eq!(
        ids(&all["data"], "id"),
        BTreeSet::from(["alice-a".into(), "alice-b".into(), "alice-c".into()])
    );
    assert_private(&all);
    for peer in all["data"].as_array().unwrap() {
        assert_eq!(peer["user"], alice.id);
        assert_eq!(peer["user_name"], "alice");
        assert_eq!(peer["info"]["os"], "Linux");
        assert_eq!(
            peer["info"]["device_name"],
            format!("Name {}", peer["id"].as_str().unwrap())
        );
    }
    let mut paged = Vec::new();
    for current in 1..=3 {
        let (status, _, page) = client(
            &app,
            &alice_token,
            "GET",
            &format!("/api/peers?current={current}&pageSize=2"),
            Value::Null,
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], 3);
        assert_eq!(
            page["data"].as_array().unwrap().len(),
            match current {
                1 => 2,
                2 => 1,
                _ => 0,
            }
        );
        paged.extend(page["data"].as_array().unwrap().iter().cloned());
    }
    assert_eq!(Value::Array(paged), all["data"]);
    let (_, _, other) = client(&app, &bob_token, "GET", "/api/peers", Value::Null).await?;
    assert_eq!(other["total"], 1);
    assert_eq!(other["data"][0]["id"], "bob-a");
    assert_eq!(other["data"][0]["user"], bob.id);
    assert_private(&other);
    for (token, user_id) in [(&alice_token, &alice.id), (&bob_token, &bob.id)] {
        let (status, _, users) = client(&app, token, "GET", "/api/users", Value::Null).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(users["total"], 1);
        assert_eq!(ids(&users["data"], "id"), BTreeSet::from([user_id.clone()]));
        assert_eq!(
            client(
                &app,
                token,
                "GET",
                "/api/users?current=2&pageSize=1",
                Value::Null
            )
            .await?
            .2,
            json!({"total":1,"data":[]})
        );
        assert_eq!(
            client(
                &app,
                token,
                "GET",
                "/api/device-group/accessible",
                Value::Null
            )
            .await?
            .2,
            json!({"total":0,"data":[]})
        );
    }
    for path in ["/api/peers", "/api/users"] {
        for query in ["current=0", "pageSize=0", "pageSize=1001"] {
            assert_eq!(
                client(
                    &app,
                    &alice_token,
                    "GET",
                    &format!("{path}?{query}"),
                    Value::Null
                )
                .await?
                .0,
                StatusCode::BAD_REQUEST
            );
        }
    }
    assert_eq!(
        client(&app, &alice_token, "POST", "/api/logout", Value::Null)
            .await?
            .0,
        StatusCode::OK
    );
    for path in ["/api/peers", "/api/users", "/api/device-group/accessible"] {
        assert_eq!(
            request(&app, "GET", path, &[], Value::Null).await?.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client(&app, &alice_token, "GET", path, Value::Null)
                .await?
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client(&app, &bob_token, "GET", path, Value::Null).await?.0,
            StatusCode::OK
        );
    }
    Ok(())
}

#[tokio::test]
async fn browser_device_assignment_requires_admin_csrf_and_revokes_old_device_sessions()
-> Result<()> {
    let store = Store::connect("sqlite::memory:").await?;
    let admin_user = store.bootstrap_admin("admin", PASSWORD).await?;
    let alice = store.create_local("alice", PASSWORD, "member").await?;
    let bob = store.create_local("bob", PASSWORD, "member").await?;
    let admin_web = store.issue_web_session(&admin_user.id, 3600).await?;
    let alice_web = store.issue_web_session(&alice.id, 3600).await?;
    let bob_web = store.issue_web_session(&bob.id, 3600).await?;
    let app = compat::router(store.clone(), 3600, None).merge(admin::router(WebAuth::new(
        store.clone(),
        3600,
        ORIGIN,
    )?));
    let alice_client = login(&app, "alice", "alice-a").await?;
    let alice_second = login(&app, "alice", "alice-a").await?;
    let alice_other_device = login(&app, "alice", "alice-b").await?;
    let bob_client = login(&app, "bob", "bob-a").await?;
    let path = "/admin/api/v1/devices";
    let (status, headers, all) = web(&app, &admin_web, "GET", path, Value::Null).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(all["devices"].as_array().unwrap().len(), 3);
    assert_private(&all);
    let owned = web(&app, &alice_web, "GET", path, Value::Null).await?.2;
    assert_eq!(
        ids(&owned["devices"], "rustdesk_id"),
        BTreeSet::from(["alice-a".into(), "alice-b".into()])
    );
    assert!(
        owned["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|device| device["owner_user_id"] == alice.id)
    );
    let target = owned["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|device| device["rustdesk_id"] == "alice-a")
        .unwrap();
    assert_eq!(target["device_name"], "Name alice-a");
    assert_eq!(target["platform"], "Linux");
    assert_eq!(target["device_type"], "desktop");
    let target_path = format!("{path}/{}", target["id"].as_str().unwrap());
    let assign = json!({"owner_user_id":bob.id});
    for headers in [
        vec![],
        vec![("authorization", format!("Bearer {admin_web}"))],
        vec![("cookie", format!("rd_web_session={alice_client}"))],
    ] {
        assert_eq!(
            request(&app, "GET", path, &headers, Value::Null).await?.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        web(&app, &alice_web, "PATCH", &target_path, assign.clone())
            .await?
            .0,
        StatusCode::FORBIDDEN
    );
    for (origin, csrf) in [
        (Some(ORIGIN), None),
        (None, Some(csrf_token(&admin_web))),
        (
            Some("https://attacker.example"),
            Some(csrf_token(&admin_web)),
        ),
        (Some(ORIGIN), Some("incorrect".into())),
    ] {
        let mut headers = vec![("cookie", format!("rd_web_session={admin_web}"))];
        if let Some(origin) = origin {
            headers.push(("origin", origin.into()));
        }
        if let Some(csrf) = csrf {
            headers.push(("x-csrf-token", csrf));
        }
        assert_eq!(
            request(&app, "PATCH", &target_path, &headers, assign.clone())
                .await?
                .0,
            StatusCode::FORBIDDEN
        );
    }
    for invalid in [
        json!({}),
        json!({"owner_user_id":17}),
        json!({"owner_user_id":""}),
        json!({"owner_user_id":bob.id,"unexpected":true}),
    ] {
        let response = web(&app, &admin_web, "PATCH", &target_path, invalid).await?;
        assert!(
            response.0.is_client_error(),
            "invalid assignment accepted: {:?}",
            response.2
        );
    }
    assert_eq!(
        web(&app, &alice_web, "GET", path, Value::Null).await?.2,
        owned
    );
    assert_eq!(
        client(&app, &alice_client, "POST", "/api/currentUser", Value::Null)
            .await?
            .0,
        StatusCode::OK
    );

    let (status, _, assigned) = web(&app, &admin_web, "PATCH", &target_path, assign).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assigned["device"]["owner_user_id"], bob.id);
    assert_private(&assigned);
    for token in [&alice_client, &alice_second] {
        for (method, path) in [
            ("POST", "/api/currentUser"),
            ("GET", "/api/peers"),
            ("GET", "/api/ab"),
        ] {
            assert_eq!(
                client(&app, token, method, path, Value::Null).await?.0,
                StatusCode::UNAUTHORIZED
            );
        }
    }
    assert_eq!(
        client(
            &app,
            &alice_other_device,
            "POST",
            "/api/currentUser",
            Value::Null
        )
        .await?
        .0,
        StatusCode::OK
    );
    let remaining = web(&app, &alice_web, "GET", path, Value::Null).await?.2;
    assert_eq!(
        ids(&remaining["devices"], "rustdesk_id"),
        BTreeSet::from(["alice-b".into()])
    );
    let bob_devices = web(&app, &bob_web, "GET", path, Value::Null).await?.2;
    assert_eq!(
        ids(&bob_devices["devices"], "rustdesk_id"),
        BTreeSet::from(["alice-a".into(), "bob-a".into()])
    );
    assert_eq!(
        client(&app, &bob_client, "GET", "/api/peers", Value::Null)
            .await?
            .2["total"],
        2
    );

    // Explicit null unassigns; signing in as the old owner cannot reclaim the device.
    let (status, _, unassigned) = web(
        &app,
        &admin_web,
        "PATCH",
        &target_path,
        json!({"owner_user_id":null}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(unassigned["device"]["owner_user_id"].is_null());
    let new_alice = login(&app, "alice", "alice-a").await?;
    assert_eq!(
        client(&app, &new_alice, "GET", "/api/peers", Value::Null)
            .await?
            .2["total"],
        1
    );
    let all = web(&app, &admin_web, "GET", path, Value::Null).await?.2;
    assert!(
        all["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|device| device["rustdesk_id"] == "alice-a")
            .unwrap()["owner_user_id"]
            .is_null()
    );
    Ok(())
}
