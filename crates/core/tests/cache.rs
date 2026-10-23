//! The cache keeps a value while its namespaces stand still, forgets it on
//! a bump, shares one load between concurrent callers, and hands out locks.
//! Runs on the database matrix for the shared signals. Imports only the
//! public API.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use laterite_core::cache::{records, CacheModule, DbSignals, MemoryStore, NullStore};
use laterite_core::parameters::{Parameters, ParametersModule};
use laterite_core::{Cache, CoreResult, Module};

async fn test_db() -> (laterite_core::Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[CacheModule.migrations(), ParametersModule.migrations()])
        .await
}

const TTL: Duration = Duration::from_secs(60);

#[tokio::test]
async fn a_value_is_kept_until_its_namespace_moves() {
    let cache = Cache::local();
    cache.allow("acme.blog::feed");
    cache
        .put("feed", b"v1".to_vec(), &["acme.blog::feed"], TTL)
        .await
        .unwrap();
    assert_eq!(cache.get("feed").await.as_deref(), Some(&b"v1"[..]));
    cache.bump("acme.blog::feed").await.unwrap();
    assert_eq!(cache.get("feed").await, None);
    // A value with no dependencies lives by its lifetime alone.
    cache.put("plain", b"x".to_vec(), &[], TTL).await.unwrap();
    cache.bump("acme.blog::feed").await.unwrap();
    assert!(cache.get("plain").await.is_some());
    assert!(cache.forget("plain").await);
    assert!(cache.get("plain").await.is_none());
}

#[tokio::test]
async fn a_lifetime_ends_a_value() {
    let cache = Cache::local();
    cache
        .put("short", b"x".to_vec(), &[], Duration::from_millis(50))
        .await
        .unwrap();
    assert!(cache.get("short").await.is_some());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(cache.get("short").await.is_none());
}

#[tokio::test]
async fn add_stores_only_once() {
    let cache = Cache::local();
    assert!(cache.add("once", b"a".to_vec(), &[], TTL).await.unwrap());
    assert!(!cache.add("once", b"b".to_vec(), &[], TTL).await.unwrap());
    assert_eq!(cache.get("once").await.as_deref(), Some(&b"a"[..]));
}

#[tokio::test]
async fn concurrent_callers_share_one_load() {
    let cache = Cache::local();
    let loads = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let cache = cache.clone();
        let loads = Arc::clone(&loads);
        tasks.push(tokio::spawn(async move {
            cache
                .remember("menu", TTL, &[&records("posts")], || async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    CoreResult::Ok(vec!["home".to_string(), "about".to_string()])
                })
                .await
                .unwrap()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), vec!["home", "about"]);
    }
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    // A failed load stores nothing and reaches the caller.
    let err = cache
        .remember::<String, _, _>("fails", TTL, &[], || async {
            Err(laterite_core::CoreError::Internal("no".into()))
        })
        .await
        .unwrap_err();
    assert!(matches!(err, laterite_core::CoreError::Internal(_)));
    assert!(cache.get("fails").await.is_none());
}

#[tokio::test]
async fn the_null_store_keeps_nothing_and_still_signals() {
    let (db, _guard) = test_db().await;
    let cache = Cache::new(NullStore, DbSignals::new(db.clone()));
    cache.allow("acme.blog::feed");
    cache
        .put("feed", b"v1".to_vec(), &["acme.blog::feed"], TTL)
        .await
        .unwrap();
    assert!(cache.get("feed").await.is_none());
    let before = cache.generation("acme.blog::feed").await.unwrap();
    assert_eq!(cache.bump("acme.blog::feed").await.unwrap(), before + 1);
    assert_eq!(cache.store(), "null");
}

#[tokio::test]
async fn another_process_sees_a_bump_on_refresh() {
    let (db, _guard) = test_db().await;
    let first = Cache::new(MemoryStore::new(), DbSignals::new(db.clone()));
    let second = Cache::new(MemoryStore::new(), DbSignals::new(db.clone()));
    for cache in [&first, &second] {
        cache.allow("acme.blog::feed");
        cache
            .put("feed", b"v1".to_vec(), &["acme.blog::feed"], TTL)
            .await
            .unwrap();
    }
    first.bump("acme.blog::feed").await.unwrap();
    // The second process still trusts its replica until it polls.
    assert!(second.get("feed").await.is_some());
    second.refresh().await.unwrap();
    assert!(second.get("feed").await.is_none());
    assert_eq!(
        first.current("acme.blog::feed"),
        second.current("acme.blog::feed")
    );
    let known = first.known().await.unwrap();
    assert!(known.iter().any(|(ns, _)| ns == "acme.blog::feed"));
    // A seed is never zero, so a generation and a reset store never collide.
    assert!(known.iter().all(|(_, g)| *g > 0));
}

#[tokio::test]
async fn bump_all_moves_every_known_namespace() {
    let (db, _guard) = test_db().await;
    let cache = Cache::new(MemoryStore::new(), DbSignals::new(db.clone()));
    cache.allow("acme.blog::feed");
    cache
        .put("feed", b"v1".to_vec(), &["acme.blog::feed"], TTL)
        .await
        .unwrap();
    cache
        .put("posts", b"v1".to_vec(), &[&records("posts")], TTL)
        .await
        .unwrap();
    assert!(cache.bump_all().await.unwrap() >= 2);
    assert!(cache.get("feed").await.is_none());
    assert!(cache.get("posts").await.is_none());
}

#[tokio::test]
async fn a_lock_is_held_by_one_owner_until_released_or_lapsed() {
    let (db, _guard) = test_db().await;
    let cache = Cache::new(NullStore, DbSignals::new(db.clone()));
    let held = cache
        .lock("sweep", TTL)
        .await
        .unwrap()
        .expect("first holder");
    assert!(cache.lock("sweep", TTL).await.unwrap().is_none());
    assert!(held.release().await.unwrap());
    let again = cache
        .lock("sweep", Duration::from_millis(50))
        .await
        .unwrap();
    assert!(again.is_some());
    tokio::time::sleep(Duration::from_millis(120)).await;
    // The lapsed lock is taken over, and the old holder's release is nothing.
    let third = cache
        .lock("sweep", TTL)
        .await
        .unwrap()
        .expect("after lapse");
    assert!(!again.unwrap().release().await.unwrap());
    assert!(third.release().await.unwrap());
}

#[tokio::test]
async fn the_local_lock_behaves_the_same() {
    let cache = Cache::local();
    let held = cache.lock("job", TTL).await.unwrap().expect("first holder");
    assert!(cache.lock("job", TTL).await.unwrap().is_none());
    assert!(held.release().await.unwrap());
    assert!(cache.lock("job", TTL).await.unwrap().is_some());
}

#[tokio::test]
async fn parameters_handles_on_one_cache_see_each_others_writes() {
    let (db, _guard) = test_db().await;
    let cache = Cache::new(MemoryStore::new(), DbSignals::new(db.clone()));
    let one = Parameters::with_cache(db.clone(), cache.clone());
    let two = Parameters::with_cache(db.clone(), cache);
    let key = "acme.blog::sync.cursor";
    assert_eq!(two.get::<u32>(key).await.unwrap(), None);
    one.set(key, &7u32).await.unwrap();
    assert_eq!(two.get::<u32>(key).await.unwrap(), Some(7));
    one.reset(key).await.unwrap();
    assert_eq!(two.get::<u32>(key).await.unwrap(), None);
    // A write behind the handles' backs is read once a handle is told, and
    // what it reads is shared.
    laterite_core::parameters::set(&db, key, &9u32)
        .await
        .unwrap();
    assert_eq!(two.get::<u32>(key).await.unwrap(), None);
    two.forget(key);
    assert_eq!(two.get::<u32>(key).await.unwrap(), Some(9));
    assert_eq!(one.get::<u32>(key).await.unwrap(), Some(9));
    // A cleared handle leaves every cached entry behind; the other keeps them.
    laterite_core::parameters::set(&db, key, &11u32)
        .await
        .unwrap();
    one.clear();
    assert_eq!(one.get::<u32>(key).await.unwrap(), Some(11));
    assert_eq!(two.get::<u32>(key).await.unwrap(), Some(9));
}
