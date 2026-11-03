use anyhow::Result;
use rustdesk_account_api::{credentials::CredentialCipher, store::Store};
use serde_json::{Value, json};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use std::sync::Arc;

const HASH: &str = "AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=";
const OTHER_HASH: &str = "BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ=";
const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const WRONG_KEY: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";

fn cipher(key: &str) -> Result<Arc<CredentialCipher>> {
    Ok(Arc::new(CredentialCipher::from_json(
        &json!({"active_key":"v1","keys":{"v1":key}}).to_string(),
    )?))
}

async fn credential_count(pool: &AnyPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM book_credentials")
        .fetch_one(pool)
        .await?)
}

async fn contract(url: &str) -> Result<()> {
    let store = Store::connect(url).await?;
    let user = store
        .bootstrap_admin("owner", "credential-test-password")
        .await?;
    let another = store
        .create_local("other", "credential-test-password", "member")
        .await?;
    let token = store.issue_session(&user.id, 3600).await?;
    let other_token = store.issue_session(&another.id, 3600).await?;
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await?;
    let snapshot = json!({"peers":[{"id":"peer-1","alias":"工作机","hash":HASH,"password":"never-store-this-password"}],"tags":["work"],"revision":7});
    // Metadata-only mode discards all password/hash fields, including nested ones.
    store.put_book_authenticated(&token, &snapshot).await?;
    assert!(
        !store
            .get_book_authenticated(&token)
            .await?
            .to_string()
            .contains(HASH)
    );
    assert_eq!(credential_count(&pool).await?, 0);
    let store = store
        .configure_credential_cipher(Some(cipher(KEY)?))
        .await?;
    assert!(store.credential_sync_enabled());
    store.put_book_authenticated(&token, &snapshot).await?;
    let read = store.get_book_authenticated(&token).await?;
    assert_eq!(read["peers"][0]["hash"], HASH);
    assert!(read["peers"][0].get("password").is_none());
    assert_eq!(read["revision"], 7);
    assert_eq!(credential_count(&pool).await?, 1);
    let metadata: String = sqlx::query_scalar(
        "SELECT snapshot FROM books WHERE user_id = (SELECT id FROM users WHERE username='owner')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        !metadata.contains(HASH)
            && !metadata.contains("never-store-this-password")
            && !metadata.contains("hash")
    );
    let row = sqlx::query("SELECT nonce,ciphertext FROM book_credentials")
        .fetch_one(&pool)
        .await?;
    assert_ne!(row.try_get::<String, _>("ciphertext")?, HASH);
    let initial_nonce: String = row.try_get("nonce")?;
    assert!(
        store.get_book_authenticated(&other_token).await?["peers"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Both trusted and authenticated methods transparently recover secrets on restart.
    let reopened = Store::connect(url)
        .await?
        .configure_credential_cipher(Some(cipher(KEY)?))
        .await?;
    assert_eq!(reopened.get_book(&user.id).await?["peers"][0]["hash"], HASH);
    assert_eq!(
        reopened.get_book_authenticated(&token).await?["peers"][0]["hash"],
        HASH
    );
    let metadata_only = Store::connect(url).await?;
    assert!(metadata_only.get_book(&user.id).await.is_err());
    assert!(metadata_only.get_book_authenticated(&token).await.is_err());
    assert!(
        metadata_only
            .put_book(&user.id, &json!({"peers":[],"tags":[]}))
            .await
            .is_err()
    );
    assert!(
        metadata_only
            .put_book_authenticated(&token, &json!({"peers":[],"tags":[]}))
            .await
            .is_err()
    );
    assert!(
        Store::connect(url)
            .await?
            .configure_credential_cipher(None)
            .await
            .is_err()
    );
    assert!(
        Store::connect(url)
            .await?
            .configure_credential_cipher(Some(cipher(WRONG_KEY)?))
            .await
            .is_err()
    );
    assert_eq!(credential_count(&pool).await?, 1);

    // A rejected upload cannot partially change metadata or erase the old secret.
    assert!(
        reopened
            .put_book(
                &user.id,
                &json!({"peers":[{"id":"peer-1","hash":"invalid"}],"tags":["changed"]})
            )
            .await
            .is_err()
    );
    assert_eq!(reopened.get_book(&user.id).await?, read);
    reopened.put_book(&user.id, &snapshot).await?;
    let nonce: String = sqlx::query_scalar("SELECT nonce FROM book_credentials")
        .fetch_one(&pool)
        .await?;
    assert_ne!(nonce, initial_nonce);

    // Whole-snapshot semantics: missing/empty hash and removed peers clear secrets.
    for peers in [
        json!([{"id":"peer-1"}]),
        json!([{"id":"peer-1","hash":""}]),
        json!([]),
    ] {
        reopened.put_book(&user.id, &snapshot).await?;
        reopened
            .put_book_authenticated(&token, &json!({"peers":peers,"tags":[]}))
            .await?;
        assert_eq!(credential_count(&pool).await?, 0);
        assert!(
            !reopened
                .get_book(&user.id)
                .await?
                .to_string()
                .contains(HASH)
        );
    }
    // Snapshot and credential replacement are atomic when SQL rejects the ciphertext.
    reopened.put_book(&user.id, &snapshot).await?;
    if url.starts_with("sqlite:") {
        sqlx::raw_sql("CREATE TRIGGER reject_book_credential BEFORE INSERT ON book_credentials BEGIN SELECT RAISE(ABORT, 'test rejection'); END;").execute(&pool).await?;
    } else {
        sqlx::raw_sql("CREATE FUNCTION reject_book_credential() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test rejection'; END; $$; CREATE TRIGGER reject_book_credential BEFORE INSERT ON book_credentials FOR EACH ROW EXECUTE FUNCTION reject_book_credential();").execute(&pool).await?;
    }
    let replacement = json!({"peers":[{"id":"peer-2","hash":OTHER_HASH}],"tags":["replacement"]});
    assert!(reopened.put_book(&user.id, &replacement).await.is_err());
    assert_eq!(reopened.get_book(&user.id).await?, read);
    sqlx::query(if url.starts_with("sqlite:") {
        "DROP TRIGGER reject_book_credential"
    } else {
        "DROP TRIGGER reject_book_credential ON book_credentials"
    })
    .execute(&pool)
    .await?;

    // Ciphertexts cannot be copied to another owner: AEAD owner binding rejects it.
    sqlx::query(if url.starts_with("sqlite:") {
        "INSERT INTO book_credentials(user_id,peer_id,key_id,nonce,ciphertext) SELECT ?,peer_id,key_id,nonce,ciphertext FROM book_credentials"
    } else {
        "INSERT INTO book_credentials(user_id,peer_id,key_id,nonce,ciphertext) SELECT $1,peer_id,key_id,nonce,ciphertext FROM book_credentials"
    }).bind(&another.id).execute(&pool).await?;
    assert!(reopened.get_book(&another.id).await.is_err());
    assert!(reopened.put_book(&another.id, &Value::Null).await.is_err());
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn sqlite_encrypted_book_credentials_and_fail_closed_modes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    contract(&format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("credentials.sqlite").display()
    ))
    .await
}

#[tokio::test]
async fn postgres_book_credentials_in_isolated_schema() -> Result<()> {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        return Ok(());
    };
    anyhow::ensure!(
        database_url.starts_with("postgres:") || database_url.starts_with("postgresql:"),
        "optional credential tests require PostgreSQL"
    );
    sqlx::any::install_default_drivers();
    let control: AnyPool = AnyPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    let schema = format!("credential_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&control)
        .await?;
    let result = async {
        let mut scoped = url::Url::parse(&database_url)?;
        scoped
            .query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        contract(scoped.as_str()).await
    }
    .await;
    let cleanup = sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&control)
        .await;
    control.close().await;
    result?;
    cleanup?;
    Ok(())
}
