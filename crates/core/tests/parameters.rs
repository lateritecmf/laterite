//! The parameters store round-trips typed values on every backend, and the
//! cached handle sees its own writes at once. Imports only the public API.

use laterite_core::parameters::{self, Parameters, ParametersModule};
use laterite_core::Module;
use serde::{Deserialize, Serialize};
use std::time::Duration;

async fn test_db() -> (laterite_core::Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[ParametersModule.migrations()]).await
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Cursor {
    page: u32,
    token: String,
}

#[tokio::test]
async fn a_parameter_round_trips_and_replaces() {
    let (db, _guard) = test_db().await;
    let key = "acme.blog::sync.cursor";
    let unset: Option<Cursor> = parameters::get(&db, key).await.unwrap();
    assert!(unset.is_none());
    let first = Cursor {
        page: 1,
        token: "a".into(),
    };
    parameters::set(&db, key, &first).await.unwrap();
    assert_eq!(
        parameters::get::<Cursor>(&db, key).await.unwrap(),
        Some(first)
    );
    let second = Cursor {
        page: 2,
        token: "b".into(),
    };
    parameters::set(&db, key, &second).await.unwrap();
    assert_eq!(
        parameters::get::<Cursor>(&db, key).await.unwrap(),
        Some(second)
    );
    assert!(parameters::reset(&db, key).await.unwrap());
    assert!(!parameters::reset(&db, key).await.unwrap());
    let gone: Option<Cursor> = parameters::get(&db, key).await.unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn keys_are_distinct_by_every_part() {
    let (db, _guard) = test_db().await;
    parameters::set(&db, "acme.blog::sync.cursor", &1)
        .await
        .unwrap();
    parameters::set(&db, "acme.shop::sync.cursor", &2)
        .await
        .unwrap();
    parameters::set(&db, "acme.blog::index.cursor", &3)
        .await
        .unwrap();
    parameters::set(&db, "acme.blog::sync.count", &4)
        .await
        .unwrap();
    assert_eq!(
        parameters::get::<i64>(&db, "acme.blog::sync.cursor")
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        parameters::get::<i64>(&db, "acme.shop::sync.cursor")
            .await
            .unwrap(),
        Some(2)
    );
    assert_eq!(
        parameters::get::<i64>(&db, "acme.blog::index.cursor")
            .await
            .unwrap(),
        Some(3)
    );
    assert_eq!(
        parameters::get::<i64>(&db, "acme.blog::sync.count")
            .await
            .unwrap(),
        Some(4)
    );
}

#[tokio::test]
async fn a_malformed_key_is_an_error_not_a_row() {
    let (db, _guard) = test_db().await;
    assert!(parameters::set(&db, "sync.cursor", &1).await.is_err());
    assert!(parameters::get::<i64>(&db, "acme::cursor").await.is_err());
}

#[tokio::test]
async fn the_cached_handle_sees_its_own_writes_and_ages_out_others() {
    let (db, _guard) = test_db().await;
    let key = "laterite.backend::setup.completed_at";
    let cached = Parameters::new(db.clone()).with_ttl(Duration::from_millis(200));
    assert_eq!(cached.get::<String>(key).await.unwrap(), None);
    // A write behind the cache's back is hidden until the entry ages out.
    parameters::set(&db, key, "2026-10-22T10:00:00Z")
        .await
        .unwrap();
    assert_eq!(cached.get::<String>(key).await.unwrap(), None);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        cached.get::<String>(key).await.unwrap().as_deref(),
        Some("2026-10-22T10:00:00Z")
    );
    // A write through the handle is seen at once, as is a reset.
    cached.set(key, "2026-10-23T10:00:00Z").await.unwrap();
    assert_eq!(
        cached.get::<String>(key).await.unwrap().as_deref(),
        Some("2026-10-23T10:00:00Z")
    );
    assert!(cached.reset(key).await.unwrap());
    assert_eq!(cached.get::<String>(key).await.unwrap(), None);
    // Forgetting a key reads the table again.
    parameters::set(&db, key, "x").await.unwrap();
    cached.forget(key);
    assert_eq!(
        cached.get::<String>(key).await.unwrap().as_deref(),
        Some("x")
    );
    assert_eq!(
        cached
            .get_or::<u32>("acme.blog::sync.page", 7)
            .await
            .unwrap(),
        7
    );
}
