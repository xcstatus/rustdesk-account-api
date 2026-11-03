use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use rustdesk_account_api::{compat, credentials::CredentialCipher, store::Store};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    app: &Router,
    method: &str,
    token: Option<&str>,
    path: &str,
    body: Value,
) -> Result<(StatusCode, HeaderMap, Value)> {
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
    let bytes = response.into_body().collect().await?.to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, headers, body))
}

async fn login(app: &Router, username: &str, device: &str) -> Result<String> {
    let (status, _, body) = request(
        app,
        "POST",
        None,
        "/api/login",
        json!({
            "username":username,"password":"credential-http-password","type":"account",
            "id":device,"uuid":format!("private-{device}"),
            "deviceInfo":{"os":"Linux","name":device,"type":"desktop"}
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    Ok(body["access_token"]
        .as_str()
        .context("login token missing")?
        .to_owned())
}

async fn write(app: &Router, token: &str, snapshot: &Value) -> Result<StatusCode> {
    let (status, headers, _) = request(
        app,
        "POST",
        Some(token),
        "/api/ab",
        json!({"data":snapshot.to_string()}),
    )
    .await?;
    assert_eq!(headers["cache-control"], "no-store");
    if status == StatusCode::OK {
        assert_eq!(
            headers["x-rustdesk-credential-sync"],
            "enabled; personal-salted-hash"
        );
    }
    Ok(status)
}

async fn read(app: &Router, token: &str) -> Result<Value> {
    let (status, headers, body) = request(app, "GET", Some(token), "/api/ab", Value::Null).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(body["licensed_devices"], 0);
    serde_json::from_str(
        body["data"]
            .as_str()
            .context("legacy data must be a JSON string")?,
    )
    .context("invalid legacy address book")
}

#[tokio::test]
async fn personal_credentials_sync_between_clients_without_crossing_account_boundaries()
-> Result<()> {
    let directory = tempfile::tempdir()?;
    let url = format!(
        "sqlite://{}",
        directory.path().join("credentials-http.sqlite").display()
    );
    let key_config =
        json!({"active_key":"test-v1","keys":{"test-v1":STANDARD.encode([1;32])}}).to_string();
    let store = Store::connect(&url)
        .await?
        .configure_credential_cipher(Some(Arc::new(CredentialCipher::from_json(&key_config)?)))
        .await?;
    let alice = store
        .create_local("alice", "credential-http-password", "member")
        .await?;
    let bob = store
        .create_local("bob", "credential-http-password", "member")
        .await?;
    let app = compat::router(store, 3600, None);
    let first = login(&app, "alice", "alice-laptop").await?;
    let second = login(&app, "alice", "alice-desktop").await?;
    let other = login(&app, "bob", "bob-laptop").await?;
    assert_ne!(first, second);
    let hash = STANDARD.encode([3; 32]);
    let other_hash = STANDARD.encode([4; 32]);
    let upload = json!({
        "peers":[{"id":"remote-1","alias":"工作机","platform":"Mac OS",
            "tags":["work"],"hash":hash,"password":"must-never-be-returned"}],
        "tags":["work"],"tag_colors":"{\"work\":4281558681}","user_id":bob.id
    });
    assert_eq!(write(&app, &first, &upload).await?, StatusCode::OK);
    let snapshot = read(&app, &second).await?;
    assert_eq!(snapshot["peers"][0]["hash"], hash);
    assert_eq!(snapshot["peers"][0]["alias"], "工作机");
    assert_eq!(snapshot["tag_colors"], upload["tag_colors"]);
    assert!(snapshot["peers"][0].get("password").is_none());
    assert!(snapshot.get("user_id").is_none());
    assert_eq!(read(&app, &other).await?, json!({"peers":[],"tags":[]}));

    // Identical peer IDs remain personal, even when an upload names another owner.
    let bob_book =
        json!({"peers":[{"id":"remote-1","hash":other_hash}],"tags":[],"user_id":alice.id});
    assert_eq!(write(&app, &other, &bob_book).await?, StatusCode::OK);
    assert_eq!(read(&app, &first).await?, snapshot);
    assert_eq!(read(&app, &other).await?["peers"][0]["hash"], other_hash);

    // Canonical standard base64 without padding is also used by official clients.
    let updated_hash = STANDARD.encode([5; 32]).trim_end_matches('=').to_owned();
    let update =
        json!({"peers":[{"id":"remote-1","alias":"updated","hash":updated_hash}],"tags":[]});
    assert_eq!(write(&app, &second, &update).await?, StatusCode::OK);
    let updated = read(&app, &first).await?;
    assert_eq!(updated, update);
    let reopened = Store::connect(&url)
        .await?
        .configure_credential_cipher(Some(Arc::new(CredentialCipher::from_json(&key_config)?)))
        .await?;
    assert_eq!(
        read(&compat::router(reopened, 3600, None), &second).await?,
        updated
    );

    // Validation must finish before replacing either metadata or encrypted hashes.
    let mut invalid_hashes = vec![
        json!("not-a-hash"),
        json!(STANDARD.encode([6; 31])),
        json!(17),
    ];
    invalid_hashes.push(json!(STANDARD.encode([255; 32]).replace('/', "_")));
    for invalid_hash in invalid_hashes {
        let invalid = json!({"peers":[{"id":"remote-1","alias":"corrupted","hash":invalid_hash}],"tags":["corrupted"]});
        assert_eq!(
            write(&app, &first, &invalid).await?,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(read(&app, &second).await?, updated);
    }
    let duplicate = json!({"peers":[{"id":"remote-1","hash":hash},{"id":"remote-1","hash":other_hash}],"tags":[]});
    assert_eq!(
        write(&app, &first, &duplicate).await?,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(read(&app, &second).await?, updated);

    // Credential peer IDs retain the legacy metadata contract through 256 bytes.
    for length in [129, 256] {
        let boundary = json!({"peers":[{"id":"p".repeat(length),"hash":hash}],"tags":[]});
        assert_eq!(write(&app, &second, &boundary).await?, StatusCode::OK);
        assert_eq!(read(&app, &first).await?, boundary);
        let too_long = json!({"peers":[{"id":"p".repeat(257),"hash":other_hash}],"tags":[]});
        assert_eq!(
            write(&app, &second, &too_long).await?,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(read(&app, &first).await?, boundary);
    }
    assert_eq!(write(&app, &second, &update).await?, StatusCode::OK);

    assert_eq!(
        request(&app, "POST", Some(&first), "/api/logout", Value::Null)
            .await?
            .0,
        StatusCode::OK
    );
    for method in ["GET", "POST"] {
        assert_eq!(
            request(
                &app,
                method,
                Some(&first),
                "/api/ab",
                json!({"data":upload.to_string()})
            )
            .await?
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &app,
                method,
                None,
                "/api/ab",
                json!({"data":upload.to_string()})
            )
            .await?
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(read(&app, &second).await?, updated);
    assert_eq!(read(&app, &other).await?["peers"][0]["hash"], other_hash);

    // Each whole snapshot explicitly clears omitted, empty, or removed credentials.
    for peers in [
        json!([{"id":"remote-1"}]),
        json!([{"id":"remote-1","hash":""}]),
        json!([]),
    ] {
        assert_eq!(write(&app, &second, &update).await?, StatusCode::OK);
        assert_eq!(
            write(&app, &second, &json!({"peers":peers,"tags":[]})).await?,
            StatusCode::OK
        );
        let cleared = read(&app, &second).await?;
        assert!(
            cleared["peers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|peer| peer.get("hash").is_none())
        );
        assert_eq!(read(&app, &other).await?["peers"][0]["hash"], other_hash);
    }
    Ok(())
}
