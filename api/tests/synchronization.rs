use anyhow::Result;
use rustdesk_account_api::store::{AccountError, Store};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Barrier;

fn assert_unauthorized<T>(result: Result<T>) {
    let error = match result {
        Ok(_) => panic!("revoked client operation unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(matches!(
        error.downcast_ref::<AccountError>(),
        Some(AccountError::Unauthorized)
    ));
}

fn book(marker: &str) -> Value {
    json!({"tags":[marker],"peers":(0..100).map(|index|
        json!({"id":format!("{marker}-{index}"),"alias":marker,"tags":[marker]}))
        .collect::<Vec<_>>()})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disable_and_authorized_write_serialize_without_later_writes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let url = format!(
        "sqlite://{}",
        directory.path().join("sync.sqlite").display()
    );
    let store = Store::connect(&url).await?;
    // Independent users allow repeated races without re-enabling a revoked user.
    // Either operation may win the lock; after disable completes, no writer with
    // the previous client credential may commit another snapshot.
    for index in 0..8 {
        let user = store
            .create_local(
                &format!("racer-{index}"),
                "synthetic-test-password",
                "member",
            )
            .await?;
        let token = store.issue_session(&user.id, 3600).await?;
        let baseline = book("before-race");
        store.put_book_authenticated(&token, &baseline).await?;
        let candidate = book("racing-write");
        let barrier = Arc::new(Barrier::new(3));
        let write_store = store.clone();
        let write_token = token.clone();
        let write_barrier = barrier.clone();
        let write_candidate = candidate.clone();
        let writer = tokio::spawn(async move {
            write_barrier.wait().await;
            write_store
                .put_book_authenticated(&write_token, &write_candidate)
                .await
        });
        let disable_store = store.clone();
        let disable_id = user.id.clone();
        let disable_barrier = barrier.clone();
        let disabler = tokio::spawn(async move {
            disable_barrier.wait().await;
            disable_store.disable_user(&disable_id).await
        });
        barrier.wait().await;
        disabler.await??;
        let write_result = writer.await?;
        let committed = store.get_book(&user.id).await?;
        match write_result {
            Ok(()) => assert_eq!(committed, candidate),
            Err(error) => {
                assert_unauthorized::<()>(Err(error));
                assert_eq!(committed, baseline);
            }
        }
        assert_unauthorized(store.get_book_authenticated(&token).await);
        assert_unauthorized(
            store
                .put_book_authenticated(&token, &book("after-disable"))
                .await,
        );
        assert_eq!(store.get_book(&user.id).await?, committed);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_authorized_snapshots_are_complete_and_ordered_followup_wins() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let url = format!(
        "sqlite://{}",
        directory.path().join("sync.sqlite").display()
    );
    let store = Store::connect(&url).await?;
    let user = store
        .create_local("writer", "synthetic-test-password", "member")
        .await?;
    let first = store.issue_session(&user.id, 3600).await?;
    let second = store.issue_session(&user.id, 3600).await?;
    assert_ne!(first, second);
    for round in 0..4 {
        let baseline = book("baseline");
        let left = book(&format!("left-{round}"));
        let right = book(&format!("right-{round}"));
        store.put_book_authenticated(&first, &baseline).await?;
        let barrier = Arc::new(Barrier::new(3));
        let mut writers = Vec::new();
        for (token, snapshot) in [
            (first.clone(), left.clone()),
            (second.clone(), right.clone()),
        ] {
            let store = store.clone();
            let barrier = barrier.clone();
            writers.push(tokio::spawn(async move {
                barrier.wait().await;
                store.put_book_authenticated(&token, &snapshot).await
            }));
        }
        barrier.wait().await;
        // Reads racing the commits must see one entire committed JSON document.
        for _ in 0..8 {
            let observed = store.get_book_authenticated(&first).await?;
            assert!(observed == baseline || observed == left || observed == right);
        }
        for writer in writers {
            writer.await??;
        }
        let final_book = store.get_book_authenticated(&second).await?;
        assert!(final_book == left || final_book == right);
        let ordered = book("committed-after-both-writers");
        store.put_book_authenticated(&first, &ordered).await?;
        assert_eq!(store.get_book_authenticated(&second).await?, ordered);
    }
    Ok(())
}
