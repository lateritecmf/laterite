//! The queue stores a job, a worker claims and runs it by the job's own
//! rules (tries, backoff, timeout, retry window), the failed list keeps
//! what gave up, and inline mode runs a job as it is pushed. Runs on the
//! database matrix. Imports only the public API.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use laterite_core::cache::CacheModule;
use laterite_core::parameters::{Parameters, ParametersModule};
use laterite_core::queue::{
    DatabaseQueue, Job, JobCx, JobError, JobReg, Mode, Options, Pushed, Queue, QueueModule,
    QueueRegistry, Services, WorkOptions,
};
use laterite_core::{Cache, Events, Module, Registry};
use serde::{Deserialize, Serialize};

async fn test_db() -> (laterite_core::Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[
        QueueModule.migrations(),
        CacheModule.migrations(),
        ParametersModule.migrations(),
    ])
    .await
}

#[derive(Debug, Serialize, Deserialize)]
struct Greet {
    name: String,
}

#[async_trait]
impl Job for Greet {
    const NAME: &'static str = "acme.test::greet";
    async fn run(&self, cx: &JobCx) -> Result<(), JobError> {
        cx.parameters()
            .set("acme.test::greeted.last", &self.name)
            .await?;
        Ok(())
    }
}

/// Fails every attempt; two tries, no backoff.
#[derive(Debug, Serialize, Deserialize)]
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
}

/// Gives up at once.
#[derive(Debug, Serialize, Deserialize)]
struct Hopeless;

#[async_trait]
impl Job for Hopeless {
    const NAME: &'static str = "acme.test::hopeless";
    async fn run(&self, _cx: &JobCx) -> Result<(), JobError> {
        Err(JobError::fail("no point"))
    }
}

/// Sleeps past its timeout.
#[derive(Debug, Serialize, Deserialize)]
struct Slow;

#[async_trait]
impl Job for Slow {
    const NAME: &'static str = "acme.test::slow";
    async fn run(&self, _cx: &JobCx) -> Result<(), JobError> {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok(())
    }
    fn timeout(&self) -> Option<Duration> {
        Some(Duration::from_millis(50))
    }
    fn tries(&self) -> u32 {
        1
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Nightly;

#[async_trait]
impl Job for Nightly {
    const NAME: &'static str = "acme.test::nightly";
    async fn run(&self, _cx: &JobCx) -> Result<(), JobError> {
        Ok(())
    }
    fn queue(&self) -> String {
        "low".to_string()
    }
    fn unique_key(&self) -> Option<String> {
        Some("nightly".to_string())
    }
}

/// Never registered.
#[derive(Debug, Serialize, Deserialize)]
struct Stranger;

#[async_trait]
impl Job for Stranger {
    const NAME: &'static str = "acme.test::stranger";
    async fn run(&self, _cx: &JobCx) -> Result<(), JobError> {
        Ok(())
    }
}

fn jobs() -> Vec<JobReg> {
    let mut registry = Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("acme.test"));
    registry.add_job::<Greet>();
    registry.add_job::<Flaky>();
    registry.add_job::<Hopeless>();
    registry.add_job::<Slow>();
    registry.add_job::<Nightly>();
    registry.take::<JobReg>()
}

fn services(db: &laterite_core::Db) -> Services {
    Services {
        db: db.clone(),
        events: Events::new(db.clone()),
        cache: Cache::local(),
        parameters: Parameters::new(db.clone()),
    }
}

fn database_queue(db: &laterite_core::Db) -> Queue {
    Queue::new(
        DatabaseQueue::new(db.clone()),
        jobs(),
        services(db),
        Options {
            mode: Mode::Worker,
            queues: vec!["default".into(), "low".into()],
            retry_after: Duration::from_secs(90),
            poll: Duration::from_millis(50),
            workers: 1,
        },
    )
}

fn stop() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    rx
}

#[tokio::test]
async fn a_pushed_job_is_claimed_run_and_removed() {
    let (db, _guard) = test_db().await;
    let queue = database_queue(&db);
    let pushed = queue.push(&Greet { name: "Ada".into() }).await.unwrap();
    assert!(matches!(pushed, Pushed::Queued(_)));
    let counts = queue.counts().await.unwrap();
    assert_eq!(counts[0].pending, 1);

    let done = queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    assert_eq!(done, 1);
    assert!(queue.counts().await.unwrap().is_empty());
    let last: Option<String> = laterite_core::parameters::get(&db, "acme.test::greeted.last")
        .await
        .unwrap();
    assert_eq!(last.as_deref(), Some("Ada"));
    // Nothing left: a second pass finds no job.
    let none = queue
        .work(WorkOptions::default().stop_when_empty(true), stop())
        .await
        .unwrap();
    assert_eq!(none, 0);
}

#[tokio::test]
async fn an_unregistered_job_is_refused_at_push() {
    let (db, _guard) = test_db().await;
    let queue = database_queue(&db);
    let err = queue.push(&Stranger).await.unwrap_err();
    assert!(matches!(err, laterite_core::CoreError::Validation(_)));
}

#[tokio::test]
async fn a_failing_job_is_retried_then_lands_on_the_failed_list() {
    let (db, _guard) = test_db().await;
    let queue = database_queue(&db);
    queue.push(&Flaky).await.unwrap();
    // First attempt fails and is released with no backoff.
    queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    assert_eq!(queue.counts().await.unwrap()[0].pending, 1);
    assert!(queue.failed().await.unwrap().is_empty());
    // Second attempt is the last try: it fails for good.
    queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    let failed = queue.failed().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].name, Flaky::NAME);
    assert_eq!(failed[0].error, "attempt 2 broke");
    assert!(queue.counts().await.unwrap().is_empty());
    // Retry puts it back, forget drops it.
    assert_eq!(queue.retry(None).await.unwrap(), 1);
    assert_eq!(queue.counts().await.unwrap()[0].pending, 1);
    assert!(queue.failed().await.unwrap().is_empty());
    assert_eq!(queue.clear(None).await.unwrap(), 1);
    queue.push(&Hopeless).await.unwrap();
    queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    let failed = queue.failed().await.unwrap();
    assert_eq!(
        failed[0].error, "no point",
        "Fail skips the remaining tries"
    );
    assert_eq!(queue.forget(Some(failed[0].id)).await.unwrap(), 1);
    assert!(queue.failed().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_job_past_its_timeout_fails() {
    let (db, _guard) = test_db().await;
    let queue = database_queue(&db);
    queue.push(&Slow).await.unwrap();
    queue
        .work(WorkOptions::default().once(true), stop())
        .await
        .unwrap();
    let failed = queue.failed().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert!(
        failed[0].error.starts_with("timed out"),
        "{}",
        failed[0].error
    );
}

#[tokio::test]
async fn a_unique_job_is_pushed_once_while_it_waits_and_a_delay_is_honoured() {
    let (db, _guard) = test_db().await;
    let queue = database_queue(&db);
    assert!(matches!(
        queue.push(&Nightly).await.unwrap(),
        Pushed::Queued(_)
    ));
    assert_eq!(queue.push(&Nightly).await.unwrap(), Pushed::Duplicate);
    let counts = queue.counts().await.unwrap();
    assert_eq!(counts[0].queue, "low");
    assert_eq!(counts[0].pending, 1);
    queue
        .push_in(
            &Greet {
                name: "Later".into(),
            },
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
    // The delayed job is not due: only the nightly one runs.
    let done = queue
        .work(WorkOptions::default().stop_when_empty(true), stop())
        .await
        .unwrap();
    assert_eq!(done, 1);
    assert_eq!(queue.counts().await.unwrap()[0].pending, 1);
}

#[tokio::test]
async fn inline_mode_runs_a_job_as_it_is_pushed() {
    let (db, _guard) = test_db().await;
    let queue = Queue::inline(jobs(), services(&db));
    assert_eq!(
        queue.push(&Greet { name: "Now".into() }).await.unwrap(),
        Pushed::Ran
    );
    let last: Option<String> = laterite_core::parameters::get(&db, "acme.test::greeted.last")
        .await
        .unwrap();
    assert_eq!(last.as_deref(), Some("Now"), "the job ran on the spot");
    let err = queue.push(&Hopeless).await.unwrap_err();
    assert!(err.to_string().contains("no point"), "{err}");
}

#[tokio::test]
async fn a_worker_stops_when_told() {
    let (db, _guard) = test_db().await;
    let queue = database_queue(&db);
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handles = queue.spawn_workers(rx);
    assert_eq!(handles.len(), 1);
    queue
        .push(&Greet {
            name: "Pool".into(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(queue.counts().await.unwrap().is_empty());
    tx.send(true).unwrap();
    for handle in handles {
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("the worker returns once told to stop")
            .unwrap();
    }
    let _ = Arc::new(());
}
