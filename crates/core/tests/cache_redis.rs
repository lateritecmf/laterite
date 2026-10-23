//! The `redis` store against a live Redis, named by `LATERITE_TEST_REDIS_URL`;
//! skipped without one. Needs the `redis` feature.

#![cfg(feature = "redis")]

use std::time::Duration;

use laterite_core::cache::{records, redis};
use laterite_core::Cache;

const TTL: Duration = Duration::from_secs(60);

async fn redis_cache(prefix: &str) -> Option<Cache> {
    let url = std::env::var("LATERITE_TEST_REDIS_URL").ok()?;
    let (values, signals) = redis::connect(&url, prefix).await.expect("redis connects");
    Some(Cache::new(values, signals))
}

#[tokio::test]
async fn values_generations_and_locks_live_in_redis() {
    let Some(cache) = redis_cache("laterite-test-a").await else {
        eprintln!("LATERITE_TEST_REDIS_URL unset; skipping");
        return;
    };
    cache.flush().await;
    cache.allow("acme.blog::feed");
    cache
        .put("feed", b"v1".to_vec(), &["acme.blog::feed"], TTL)
        .await
        .unwrap();
    assert_eq!(cache.get("feed").await.as_deref(), Some(&b"v1"[..]));
    assert!(!cache.add("feed", b"v2".to_vec(), &[], TTL).await.unwrap());

    // A second handle, as another process would hold, sees the bump on refresh.
    let other = redis_cache("laterite-test-a").await.unwrap();
    other.allow("acme.blog::feed");
    assert_eq!(other.get("feed").await.as_deref(), Some(&b"v1"[..]));
    cache.bump("acme.blog::feed").await.unwrap();
    other.refresh().await.unwrap();
    assert!(other.get("feed").await.is_none());

    cache
        .put("posts", b"p".to_vec(), &[&records("posts")], TTL)
        .await
        .unwrap();
    assert!(cache.bump_all().await.unwrap() >= 2);
    assert!(cache.get("posts").await.is_none());

    let held = cache
        .lock("sweep", TTL)
        .await
        .unwrap()
        .expect("first holder");
    assert!(other.lock("sweep", TTL).await.unwrap().is_none());
    assert!(held.release().await.unwrap());
    let again = other
        .lock("sweep", Duration::from_millis(50))
        .await
        .unwrap();
    assert!(again.is_some());
    tokio::time::sleep(Duration::from_millis(120)).await;
    let third = cache
        .lock("sweep", TTL)
        .await
        .unwrap()
        .expect("after lapse");
    assert!(!again.unwrap().release().await.unwrap());
    assert!(third.release().await.unwrap());
    cache.flush().await;
}
