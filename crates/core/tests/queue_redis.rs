//! The `redis` queue driver against a live Redis, named by
//! `LATERITE_TEST_REDIS_URL`; skipped without one. Needs the `redis` feature.

#![cfg(feature = "redis")]

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use laterite_core::parameters::{Parameters, ParametersModule};
use laterite_core::queue::{
    redis, Job, JobCx, JobError, JobReg, Mode, Options, Pushed, Queue, QueueDriver, Services,
    WorkOptions,
};
use laterite_core::{Cache, Events, Module};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Greet {
    name: String,
}

#[async_trait]
impl Job for Greet {
    const NAME: &'static str = "acme.test::greet";
    async fn run(&self, cx: &JobCx) -> Result<(), JobError> {
        cx.parameters()
            .set("acme.test::greeted.redis", &self.name)
            .await?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Flaky;

#[async_trait]
impl Job for Flaky {
    const NAME: &'static str = "acme.test::flaky";
    async fn run(&self, cx: &JobCx) -> Result<(), JobError> {
        Err(JobError::retry(format!("attempt {} broke", cx.attempt())))
    }
    fn tries(&self) -> u32 {
        2
    }
    fn backoff(&self, _attempt: u32) -> Duration {
        Duration::ZERO
    }
    fn unique_key(&self) -> Option<String> {
        Some("flaky".to_string())
    }
}

fn stop() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    rx
}

#[tokio::test]
async fn jobs_round_trip_through_redis() {
    let Ok(url) = std::env::var("LATERITE_TEST_REDIS_URL") else {
        eprintln!("LATERITE_TEST_REDIS_URL unset; skipping");
        return;
    };
    let (db, _guard) = laterite_core::testing::connect_test(&[ParametersModule.migrations()]).await;
    let driver = redis::connect(&url, "laterite-test-q").await.unwrap();
    driver.clear(None).await.unwrap();
    driver.forget(None).await.unwrap();
    let queue = Queue::new(
        driver.clone(),
        vec![JobReg::of::<Greet>(), JobReg::of::<Flaky>()],
        Services {
            db: db.clone(),
            events: Events::new(db.clone()),
            cache: Cache::local(),
            parameters: Parameters::new(db.clone()),
        },
        Options {
            mode: Mode::Worker,
            queues: vec!["default".into()],
            retry_after: Duration::from_secs(90),
            poll: Duration::from_millis(50),
            workers: 1,
        },
    );

    // Push, claim, run, remove.
    assert!(matches!(
        queue.push(&Greet { name: "Ada".into() }).await.unwrap(),
        Pushed::Queued(_)
    ));
    assert_eq!(queue.counts().await.unwrap()[0].pending, 1);
    let done = queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    assert_eq!(done, 1);
    assert!(queue.counts().await.unwrap().is_empty());
    let last: Option<String> = laterite_core::parameters::get(&db, "acme.test::greeted.redis")
        .await
        .unwrap();
    assert_eq!(last.as_deref(), Some("Ada"));

    // A delayed job is not due.
    queue
        .push_in(
            &Greet {
                name: "Later".into(),
            },
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(
        queue
            .work(WorkOptions::default().stop_when_empty(true), stop())
            .await
            .unwrap(),
        0
    );
    assert_eq!(queue.clear(None).await.unwrap(), 1);

    // Unique: one waits, the second is dropped; two tries then failed.
    assert!(matches!(
        queue.push(&Flaky).await.unwrap(),
        Pushed::Queued(_)
    ));
    assert_eq!(queue.push(&Flaky).await.unwrap(), Pushed::Duplicate);
    queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    assert_eq!(
        queue.counts().await.unwrap()[0].pending,
        1,
        "released for attempt 2"
    );
    queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    let failed = queue.failed().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].error, "attempt 2 broke");
    // The unique key is free again once the job failed.
    assert!(matches!(
        queue.push(&Flaky).await.unwrap(),
        Pushed::Queued(_)
    ));
    assert_eq!(queue.clear(None).await.unwrap(), 1);
    // Retry puts the failed job back; forget drops the rest.
    assert_eq!(queue.retry(None).await.unwrap(), 1);
    assert_eq!(queue.counts().await.unwrap()[0].pending, 1);
    assert!(queue.failed().await.unwrap().is_empty());
    queue.clear(None).await.unwrap();

    // A stale claim is reclaimed.
    let short = redis::connect(&url, "laterite-test-q").await.unwrap();
    short
        .push(
            "default",
            Greet::NAME,
            &serde_json::json!({"name": "x"}),
            Utc::now(),
            None,
        )
        .await
        .unwrap();
    let first = short
        .claim(&["default".into()], Duration::from_millis(10), "w1")
        .await
        .unwrap()
        .expect("claimed");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let again = short
        .claim(&["default".into()], Duration::from_millis(10), "w2")
        .await
        .unwrap()
        .expect("reclaimed after retry_after");
    assert_eq!(again.id, first.id);
    assert_eq!(again.attempts, 2);
    short.delete(again.id).await.unwrap();
    assert!(queue.counts().await.unwrap().is_empty());
}
