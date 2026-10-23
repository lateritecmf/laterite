//! The scheduler runs what is due at a minute, once per deployment, skips
//! an entry still running, pushes a job entry onto the queue, and records
//! each run. Runs on the database matrix. Imports only the public API.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use laterite_core::cache::{CacheModule, DbSignals, NullStore};
use laterite_core::parameters::{Parameters, ParametersModule};
use laterite_core::queue::{Job, JobCx, JobError, JobReg, Queue, QueueModule};
use laterite_core::schedule::{
    Overlap, RunStatus, Schedule, ScheduleEntry, ScheduleModule, ScheduleRegistry, Services,
};
use laterite_core::{Cache, Events, Module, Registry};
use serde::{Deserialize, Serialize};

async fn test_db() -> (laterite_core::Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[
        ScheduleModule.migrations(),
        QueueModule.migrations(),
        CacheModule.migrations(),
        ParametersModule.migrations(),
    ])
    .await
}

#[derive(Serialize, Deserialize)]
struct Digest;

#[async_trait]
impl Job for Digest {
    const NAME: &'static str = "acme.test::digest";
    async fn run(&self, _cx: &JobCx) -> Result<(), JobError> {
        Ok(())
    }
}

fn services(db: &laterite_core::Db, cache: Cache) -> Services {
    let queue = Queue::new(
        laterite_core::queue::DatabaseQueue::new(db.clone()),
        vec![JobReg::of::<Digest>()],
        laterite_core::queue::Services {
            db: db.clone(),
            events: Events::new(db.clone()),
            cache: cache.clone(),
            parameters: Parameters::new(db.clone()),
        },
        laterite_core::queue::Options::default(),
    );
    Services {
        db: db.clone(),
        events: Events::new(db.clone()),
        cache,
        parameters: Parameters::new(db.clone()),
        queue,
    }
}

fn entries(hold: Arc<tokio::sync::Notify>, sweeps: Arc<AtomicU32>) -> Vec<ScheduleEntry> {
    let mut registry = Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("acme.test"));
    registry.add_schedule(
        ScheduleEntry::new("acme.test::sweep", "* * * * *").run(move |_cx| {
            let sweeps = sweeps.clone();
            async move {
                sweeps.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }),
    );
    registry.add_schedule(
        ScheduleEntry::new("acme.test::slow", "* * * * *").run(move |_cx| {
            let hold = hold.clone();
            async move {
                hold.notified().await;
                Ok(())
            }
        }),
    );
    registry.add_schedule(ScheduleEntry::daily_at("acme.test::digest", "07:00").job(Digest));
    registry.add_schedule(
        ScheduleEntry::new("acme.test::broken", "*/2 * * * *")
            .run(|_cx| async { Err(laterite_core::CoreError::Internal("boom".into())) }),
    );
    registry.take::<ScheduleEntry>()
}

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}

#[tokio::test]
async fn due_entries_run_once_per_deployment_and_are_recorded() {
    let (db, _guard) = test_db().await;
    let hold = Arc::new(tokio::sync::Notify::new());
    let sweeps = Arc::new(AtomicU32::new(0));
    let first = Schedule::new(
        entries(hold.clone(), sweeps.clone()),
        services(&db, Cache::new(NullStore, DbSignals::new(db.clone()))),
    )
    .unwrap();
    let second = Schedule::new(
        entries(hold.clone(), sweeps.clone()),
        services(&db, Cache::new(NullStore, DbSignals::new(db.clone()))),
    )
    .unwrap();
    assert_eq!(first.len(), 4);

    // An odd minute: the sweep and the slow entry are due, not the digest.
    let tick = first.tick(at("2026-10-24T10:01:30Z")).await.unwrap();
    assert_eq!(tick.ran, vec!["acme.test::sweep", "acme.test::slow"]);
    assert!(tick.skipped.is_empty());
    // The second process sees the minute taken.
    let tick = second.tick(at("2026-10-24T10:01:40Z")).await.unwrap();
    assert!(tick.ran.is_empty());
    assert_eq!(tick.skipped, vec!["acme.test::sweep", "acme.test::slow"]);
    settle().await;
    assert_eq!(sweeps.load(Ordering::SeqCst), 1);

    // Next minute: the sweep runs again; the slow one is still going and is
    // skipped, which the state records.
    let tick = first.tick(at("2026-10-24T10:02:10Z")).await.unwrap();
    assert!(tick.ran.contains(&"acme.test::sweep".to_string()));
    assert!(tick.ran.contains(&"acme.test::broken".to_string()));
    assert_eq!(tick.skipped, vec!["acme.test::slow"]);
    settle().await;
    let states = first.list().await.unwrap();
    let state = |name: &str| states.iter().find(|s| s.name == name).unwrap().clone();
    assert_eq!(state("acme.test::sweep").last_status, RunStatus::Ok);
    assert!(state("acme.test::sweep").last_finished_at.is_some());
    assert_eq!(state("acme.test::slow").last_status, RunStatus::Running);
    assert_eq!(state("acme.test::broken").last_status, RunStatus::Failed);
    assert!(state("acme.test::broken")
        .last_error
        .as_deref()
        .is_some_and(|e| e.contains("boom")));
    assert_eq!(state("acme.test::digest").last_status, RunStatus::Never);
    assert_eq!(state("acme.test::digest").expression, "0 7 * * *");
    assert!(state("acme.test::digest").next_run_at.is_some());

    // Let the slow one finish: it is Ok and runs again next minute.
    hold.notify_waiters();
    settle().await;
    assert_eq!(state("acme.test::slow").name, "acme.test::slow");
    let states = first.list().await.unwrap();
    assert_eq!(
        states
            .iter()
            .find(|s| s.name == "acme.test::slow")
            .unwrap()
            .last_status,
        RunStatus::Ok
    );
}

#[tokio::test]
async fn a_job_entry_pushes_onto_the_queue_and_run_now_ignores_the_clock() {
    let (db, _guard) = test_db().await;
    let hold = Arc::new(tokio::sync::Notify::new());
    let schedule = Schedule::new(
        entries(hold, Arc::new(AtomicU32::new(0))),
        services(&db, Cache::local()),
    )
    .unwrap();
    let tick = schedule.tick(at("2026-10-24T07:00:05Z")).await.unwrap();
    assert!(tick.ran.contains(&"acme.test::digest".to_string()));
    settle().await;
    let queue = services(&db, Cache::local()).queue;
    let counts = queue.counts().await.unwrap();
    assert_eq!(counts[0].pending, 1);
    assert!(schedule.run_now("acme.test::digest").await.unwrap());
    assert_eq!(queue.counts().await.unwrap()[0].pending, 2);
    assert!(schedule.run_now("acme.test::nobody").await.is_err());
}

#[tokio::test]
async fn overlap_allow_runs_beside_a_running_one_and_a_duplicate_name_is_refused() {
    let (db, _guard) = test_db().await;
    let hold = Arc::new(tokio::sync::Notify::new());
    let entry = |name: &str| {
        let hold = hold.clone();
        ScheduleEntry::new(name, "* * * * *")
            .overlap(Overlap::Allow)
            .one_server(false)
            .run(move |_cx| {
                let hold = hold.clone();
                async move {
                    hold.notified().await;
                    Ok(())
                }
            })
    };
    let schedule = Schedule::new(
        vec![entry("acme.test::parallel")],
        services(&db, Cache::local()),
    )
    .unwrap();
    let a = schedule.tick(at("2026-10-24T10:01:00Z")).await.unwrap();
    let b = schedule.tick(at("2026-10-24T10:02:00Z")).await.unwrap();
    assert_eq!(a.ran.len() + b.ran.len(), 2);
    hold.notify_waiters();
    let dup = Schedule::new(
        vec![entry("acme.test::x"), entry("acme.test::x")],
        services(&db, Cache::local()),
    );
    assert!(dup.is_err());
}
