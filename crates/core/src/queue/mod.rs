//! The queue: work pushed now and run by a worker, with retries, a failed
//! list and a lifetime per job.
//!
//! A job is a serde struct that implements [`Job`]; a module registers it
//! by type, and a payload names it. Pushing records the job; a worker claims
//! it, runs it under its timeout, deletes it on success, releases it with
//! its backoff on failure, and moves it to the failed list once its tries or
//! its retry window are spent.
//!
//! ```ignore
//! use laterite_core::queue::{Job, JobCx, JobError, QueueRegistry};
//!
//! #[derive(Serialize, Deserialize)]
//! struct SendDigest { user_id: i64 }
//!
//! #[async_trait]
//! impl Job for SendDigest {
//!     const NAME: &'static str = "acme.blog::send_digest";
//!     async fn run(&self, cx: &JobCx) -> Result<(), JobError> {
//!         digest::send(cx.db(), self.user_id).await?;
//!         Ok(())
//!     }
//! }
//!
//! fn register(&self, registry: &mut Registry) {
//!     registry.add_job::<SendDigest>();
//! }
//!
//! ctx.queue().push(&SendDigest { user_id: 7 }).await?;
//! ```
//!
//! Jobs live in a [`QueueDriver`]: the database by default, or Redis with
//! the `redis` store. `queue.mode = inline` runs a job as it is pushed, for
//! tests and for an application without a worker.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::config::QueueConfig;
use crate::error::{CoreError, CoreResult};
use crate::events::{Event, Events};
use crate::parameters::Parameters;
use crate::registry::Registry;
use crate::{Db, Module, ModuleId};

mod database;
pub mod migrations;
#[cfg(feature = "redis")]
pub mod redis;
mod worker;

pub use database::DatabaseQueue;
pub use worker::{WorkOptions, Worker};

/// The module that owns the `jobs` and `failed_jobs` tables, listed among
/// an application's built-ins.
pub struct QueueModule;

impl Module for QueueModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> crate::MigrationSet {
        migrations::migrations()
    }
}

/// The queue a job lands on when it names none.
pub const DEFAULT_QUEUE: &str = "default";

/// Why a run did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// Try again after the backoff, while tries and the retry window allow.
    Retry(String),
    /// Move to the failed list now, whatever tries remain.
    Fail(String),
}

impl JobError {
    /// A failure worth another attempt.
    pub fn retry(message: impl std::fmt::Display) -> Self {
        Self::Retry(message.to_string())
    }

    /// A failure no attempt would mend.
    pub fn fail(message: impl std::fmt::Display) -> Self {
        Self::Fail(message.to_string())
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Retry(m) | Self::Fail(m) => m,
        }
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for JobError {}

impl From<CoreError> for JobError {
    fn from(e: CoreError) -> Self {
        Self::Retry(e.to_string())
    }
}

impl From<sqlx::Error> for JobError {
    fn from(e: sqlx::Error) -> Self {
        Self::Retry(e.to_string())
    }
}

/// What a running job sees: the application's services and its own attempt.
#[derive(Clone)]
pub struct JobCx {
    db: Db,
    events: Events,
    cache: Cache,
    parameters: Parameters,
    id: Option<i64>,
    attempt: u32,
    queue: String,
}

impl JobCx {
    pub fn db(&self) -> &Db {
        &self.db
    }
    pub fn events(&self) -> &Events {
        &self.events
    }
    pub fn cache(&self) -> &Cache {
        &self.cache
    }
    pub fn parameters(&self) -> &Parameters {
        &self.parameters
    }
    /// The job's row id; `None` when it runs inline.
    pub fn id(&self) -> Option<i64> {
        self.id
    }
    /// Which attempt this is, from 1.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
    pub fn queue(&self) -> &str {
        &self.queue
    }
}

/// Work to run later. The struct is the payload: whatever it carries is
/// stored as JSON and read back by the worker.
#[async_trait]
pub trait Job: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The job's name, unique across the application: `acme.blog::send_digest`.
    const NAME: &'static str;

    async fn run(&self, cx: &JobCx) -> Result<(), JobError>;

    /// The queue the job lands on. [`DEFAULT_QUEUE`] unless overridden.
    fn queue(&self) -> String {
        DEFAULT_QUEUE.to_string()
    }

    /// How many attempts before the job fails; `0` means as many as the
    /// retry window allows. Three unless overridden.
    fn tries(&self) -> u32 {
        3
    }

    /// How long to wait before attempt `attempt + 1`, after attempt
    /// `attempt` failed. Ten seconds, doubling each time, unless overridden.
    fn backoff(&self, attempt: u32) -> Duration {
        Duration::from_secs(10u64 << attempt.saturating_sub(1).min(8))
    }

    /// How long one attempt may run. A minute unless overridden; `None`
    /// means no limit.
    fn timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(60))
    }

    /// Keep retrying until this moment, whatever `tries` says. `None`
    /// unless overridden.
    fn retry_until(&self) -> Option<DateTime<Utc>> {
        None
    }

    /// A key no two pending jobs may share: a push while one with the same
    /// key waits is dropped. `None` unless overridden.
    fn unique_key(&self) -> Option<String> {
        None
    }
}

/// What the worker reads from a payload before running it.
#[derive(Debug, Clone)]
pub struct JobMeta {
    pub tries: u32,
    pub timeout: Option<Duration>,
    pub retry_until: Option<DateTime<Utc>>,
}

type RunFn = dyn Fn(serde_json::Value, JobCx) -> Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>
    + Send
    + Sync;
type MetaFn = dyn Fn(&serde_json::Value) -> Result<JobMeta, String> + Send + Sync;
type BackoffFn = dyn Fn(&serde_json::Value, u32) -> Duration + Send + Sync;

/// A registered job: its name and how to run a payload of it. Built by
/// [`JobReg::of`] and contributed through [`QueueRegistry::add_job`].
#[derive(Clone)]
pub struct JobReg {
    name: &'static str,
    run: Arc<RunFn>,
    meta: Arc<MetaFn>,
    backoff: Arc<BackoffFn>,
}

impl std::fmt::Debug for JobReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobReg").field("name", &self.name).finish()
    }
}

impl JobReg {
    /// The registration for `T`.
    pub fn of<T: Job>() -> Self {
        Self {
            name: T::NAME,
            run: Arc::new(|payload, cx| {
                Box::pin(async move {
                    let job: T = serde_json::from_value(payload)
                        .map_err(|e| JobError::Fail(format!("payload does not decode: {e}")))?;
                    job.run(&cx).await
                })
            }),
            meta: Arc::new(|payload| {
                let job: T = serde_json::from_value(payload.clone())
                    .map_err(|e| format!("payload does not decode: {e}"))?;
                Ok(JobMeta {
                    tries: job.tries(),
                    timeout: job.timeout(),
                    retry_until: job.retry_until(),
                })
            }),
            backoff: Arc::new(|payload, attempt| {
                serde_json::from_value::<T>(payload.clone())
                    .map(|job| job.backoff(attempt))
                    .unwrap_or(Duration::from_secs(10))
            }),
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// `registry.add_job::<T>()`: the typed way to contribute a job.
pub trait QueueRegistry {
    fn add_job<T: Job>(&mut self);
}

impl QueueRegistry for Registry {
    fn add_job<T: Job>(&mut self) {
        self.add(JobReg::of::<T>());
    }
}

/// A job as it waits or runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Queued {
    pub id: i64,
    pub queue: String,
    pub name: String,
    pub payload: serde_json::Value,
    /// How many times a worker has claimed it, this claim included.
    pub attempts: u32,
    pub available_at: String,
    pub reserved_at: Option<String>,
    pub reserved_by: Option<String>,
    pub created_at: String,
}

/// A job on the failed list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailedJob {
    pub id: i64,
    pub queue: String,
    pub name: String,
    pub payload: serde_json::Value,
    pub error: String,
    pub failed_at: String,
}

/// How many jobs a queue holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueCount {
    pub queue: String,
    /// Waiting, including those released for a later attempt.
    pub pending: u64,
    /// Claimed by a worker now.
    pub reserved: u64,
}

/// A pushed job's fate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pushed {
    /// Stored under this id.
    Queued(i64),
    /// Ran at once (`queue.mode = inline`).
    Ran,
    /// Dropped: a job with the same unique key is already waiting.
    Duplicate,
}

/// Where jobs wait. Every method is one storage operation; the worker's
/// rules live in [`Worker`].
#[async_trait]
pub trait QueueDriver: Send + Sync {
    fn name(&self) -> &'static str;
    /// Stores a job; `None` when `unique_key` names one already waiting.
    async fn push(
        &self,
        queue: &str,
        name: &str,
        payload: &serde_json::Value,
        available_at: DateTime<Utc>,
        unique_key: Option<&str>,
    ) -> CoreResult<Option<i64>>;
    /// Claims the oldest job due on one of `queues`, or one whose claim is
    /// older than `retry_after`, for `worker`. Its `attempts` count this claim.
    async fn claim(
        &self,
        queues: &[String],
        retry_after: Duration,
        worker: &str,
    ) -> CoreResult<Option<Queued>>;
    /// Puts a claimed job back, due at `available_at`.
    async fn release(&self, id: i64, available_at: DateTime<Utc>) -> CoreResult<()>;
    /// Removes a finished job.
    async fn delete(&self, id: i64) -> CoreResult<()>;
    /// Moves a job to the failed list; returns the failed row's id.
    async fn fail(&self, job: &Queued, error: &str) -> CoreResult<i64>;
    async fn counts(&self) -> CoreResult<Vec<QueueCount>>;
    async fn failed(&self) -> CoreResult<Vec<FailedJob>>;
    /// Puts a failed job (or every one, with `None`) back on its queue;
    /// returns how many moved.
    async fn retry(&self, id: Option<i64>) -> CoreResult<usize>;
    /// Drops a failed job (or every one, with `None`); returns how many went.
    async fn forget(&self, id: Option<i64>) -> CoreResult<usize>;
    /// Drops every waiting job on `queue`, or on every queue with `None`.
    async fn clear(&self, queue: Option<&str>) -> CoreResult<usize>;
    /// Checks the store answers; a driver with nothing to check says yes.
    async fn ping(&self) -> CoreResult<()> {
        Ok(())
    }
}

/// How the queue runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Workers in this process take the jobs.
    Worker,
    /// Another process (`queue work`) takes the jobs.
    External,
    /// A job runs as it is pushed; nothing is stored.
    Inline,
}

impl Mode {
    pub fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "worker" => Ok(Self::Worker),
            "external" => Ok(Self::External),
            "inline" => Ok(Self::Inline),
            other => Err(CoreError::Config(format!(
                "queue.mode `{other}` is not one of worker, external, inline"
            ))),
        }
    }
}

/// The services a job runs against.
#[derive(Clone)]
pub struct Services {
    pub db: Db,
    pub events: Events,
    pub cache: Cache,
    pub parameters: Parameters,
}

struct Inner {
    driver: Box<dyn QueueDriver>,
    jobs: HashMap<&'static str, JobReg>,
    services: Services,
    mode: Mode,
    queues: Vec<String>,
    retry_after: Duration,
    poll: Duration,
    workers: usize,
    /// Pushes wake an idle worker in this process.
    wake: tokio::sync::Notify,
}

/// The queue handle. Cheap to clone: every clone is the same queue.
#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("driver", &self.inner.driver.name())
            .field("mode", &self.inner.mode)
            .field("jobs", &self.inner.jobs.len())
            .finish_non_exhaustive()
    }
}

/// What [`Queue::new`] takes beside the driver and the jobs.
#[derive(Debug, Clone)]
pub struct Options {
    pub mode: Mode,
    /// The queues a worker takes from, in priority order.
    pub queues: Vec<String>,
    /// After this long a claimed job counts as abandoned and is claimed again.
    pub retry_after: Duration,
    /// How long an idle worker waits before looking again.
    pub poll: Duration,
    /// How many workers [`Queue::spawn_workers`] starts.
    pub workers: usize,
}

impl Options {
    /// The options `[queue]` configures.
    pub fn from_config(config: &QueueConfig) -> CoreResult<Self> {
        let mut queues = config.queues.clone();
        if queues.is_empty() {
            queues.push(DEFAULT_QUEUE.to_string());
        }
        Ok(Self {
            mode: Mode::parse(&config.mode)?,
            queues,
            retry_after: Duration::from_secs(config.retry_after_seconds.max(1)),
            poll: Duration::from_secs(config.poll_seconds.max(1)),
            workers: config.workers,
        })
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Worker,
            queues: vec![DEFAULT_QUEUE.to_string()],
            retry_after: Duration::from_secs(90),
            poll: Duration::from_secs(1),
            workers: 2,
        }
    }
}

impl Queue {
    /// A queue over `driver`, knowing `jobs`.
    pub fn new(
        driver: impl QueueDriver + 'static,
        jobs: impl IntoIterator<Item = JobReg>,
        services: Services,
        options: Options,
    ) -> Self {
        let jobs = jobs.into_iter().map(|j| (j.name, j)).collect();
        Self {
            inner: Arc::new(Inner {
                driver: Box::new(driver),
                jobs,
                services,
                mode: options.mode,
                queues: options.queues,
                retry_after: options.retry_after,
                poll: options.poll,
                workers: options.workers,
                wake: tokio::sync::Notify::new(),
            }),
        }
    }

    /// A queue that runs every job as it is pushed, over no storage: for
    /// tests and tools.
    pub fn inline(jobs: impl IntoIterator<Item = JobReg>, services: Services) -> Self {
        Self::new(
            NoQueue,
            jobs,
            services,
            Options {
                mode: Mode::Inline,
                ..Options::default()
            },
        )
    }

    pub fn mode(&self) -> Mode {
        self.inner.mode
    }

    pub fn driver_name(&self) -> &'static str {
        self.inner.driver.name()
    }

    /// The names of every registered job, sorted.
    pub fn job_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.inner.jobs.keys().copied().collect();
        names.sort_unstable();
        names
    }

    pub(crate) fn job(&self, name: &str) -> Option<&JobReg> {
        self.inner.jobs.get(name)
    }

    pub(crate) fn services(&self) -> &Services {
        &self.inner.services
    }

    pub(crate) fn driver(&self) -> &dyn QueueDriver {
        self.inner.driver.as_ref()
    }

    pub(crate) fn queues(&self) -> &[String] {
        &self.inner.queues
    }

    pub(crate) fn retry_after(&self) -> Duration {
        self.inner.retry_after
    }

    pub(crate) fn poll(&self) -> Duration {
        self.inner.poll
    }

    pub(crate) fn wake(&self) -> &tokio::sync::Notify {
        &self.inner.wake
    }

    fn check<T: Job>(&self) -> CoreResult<()> {
        if self.inner.jobs.contains_key(T::NAME) {
            Ok(())
        } else {
            Err(CoreError::Validation(format!(
                "job `{}` is not registered; add `registry.add_job::<{}>()` to a module",
                T::NAME,
                std::any::type_name::<T>()
            )))
        }
    }

    /// Pushes `job`, due now.
    pub async fn push<T: Job>(&self, job: &T) -> CoreResult<Pushed> {
        self.push_at(job, Utc::now()).await
    }

    /// Pushes `job`, due after `delay`.
    pub async fn push_in<T: Job>(&self, job: &T, delay: Duration) -> CoreResult<Pushed> {
        let delay = chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::MAX);
        self.push_at(job, Utc::now() + delay).await
    }

    /// Pushes `job`, due at `when`.
    pub async fn push_at<T: Job>(&self, job: &T, when: DateTime<Utc>) -> CoreResult<Pushed> {
        self.check::<T>()?;
        if self.inner.mode == Mode::Inline {
            let cx = self.context(None, 1, &job.queue());
            return match job.run(&cx).await {
                Ok(()) => Ok(Pushed::Ran),
                Err(e) => Err(CoreError::Internal(format!(
                    "job `{}` failed: {e}",
                    T::NAME
                ))),
            };
        }
        let payload = serde_json::to_value(job)
            .map_err(|e| CoreError::Internal(format!("job `{}` cannot serialize: {e}", T::NAME)))?;
        let unique = job.unique_key();
        let stored = self
            .inner
            .driver
            .push(&job.queue(), T::NAME, &payload, when, unique.as_deref())
            .await?;
        match stored {
            Some(id) => {
                self.inner.wake.notify_waiters();
                tracing::debug!(job = T::NAME, id, "job queued");
                Ok(Pushed::Queued(id))
            }
            None => Ok(Pushed::Duplicate),
        }
    }

    /// Pushes a job by its registered name with a payload already encoded,
    /// due now: what a schedule entry does. Refused when no job has the name.
    pub async fn push_named(
        &self,
        name: &str,
        payload: &serde_json::Value,
        queue: &str,
    ) -> CoreResult<Pushed> {
        let Some(reg) = self.inner.jobs.get(name) else {
            return Err(CoreError::Validation(format!(
                "job `{name}` is not registered"
            )));
        };
        if self.inner.mode == Mode::Inline {
            let cx = self.context(None, 1, queue);
            return match (reg.run)(payload.clone(), cx).await {
                Ok(()) => Ok(Pushed::Ran),
                Err(e) => Err(CoreError::Internal(format!("job `{name}` failed: {e}"))),
            };
        }
        let stored = self
            .inner
            .driver
            .push(queue, name, payload, Utc::now(), None)
            .await?;
        match stored {
            Some(id) => {
                self.inner.wake.notify_waiters();
                Ok(Pushed::Queued(id))
            }
            None => Ok(Pushed::Duplicate),
        }
    }

    pub(crate) fn context(&self, id: Option<i64>, attempt: u32, queue: &str) -> JobCx {
        let s = &self.inner.services;
        JobCx {
            db: s.db.clone(),
            events: s.events.clone(),
            cache: s.cache.clone(),
            parameters: s.parameters.clone(),
            id,
            attempt,
            queue: queue.to_string(),
        }
    }

    /// How many jobs each queue holds.
    pub async fn counts(&self) -> CoreResult<Vec<QueueCount>> {
        self.inner.driver.counts().await
    }

    /// The failed list, newest first.
    pub async fn failed(&self) -> CoreResult<Vec<FailedJob>> {
        self.inner.driver.failed().await
    }

    /// Puts one failed job, or every one, back on its queue.
    pub async fn retry(&self, id: Option<i64>) -> CoreResult<usize> {
        let moved = self.inner.driver.retry(id).await?;
        if moved > 0 {
            self.inner.wake.notify_waiters();
        }
        Ok(moved)
    }

    /// Drops one failed job, or every one.
    pub async fn forget(&self, id: Option<i64>) -> CoreResult<usize> {
        self.inner.driver.forget(id).await
    }

    /// Drops every waiting job on `queue`, or everywhere.
    pub async fn clear(&self, queue: Option<&str>) -> CoreResult<usize> {
        self.inner.driver.clear(queue).await
    }

    /// Runs jobs on this task until `stop` says so, or as `options` end it.
    /// Returns how many jobs it processed.
    pub async fn work(
        &self,
        options: WorkOptions,
        stop: tokio::sync::watch::Receiver<bool>,
    ) -> CoreResult<usize> {
        Worker::new(self.clone(), options).run(stop).await
    }

    /// Starts the configured number of workers, each taking from the
    /// configured queues, until `stop` says so.
    pub fn spawn_workers(
        &self,
        stop: tokio::sync::watch::Receiver<bool>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        (0..self.inner.workers)
            .map(|n| {
                let queue = self.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    let options = WorkOptions::default().name(format!("worker-{n}"));
                    if let Err(e) = queue.work(options, stop).await {
                        tracing::error!(worker = n, error = %e, "worker stopped");
                    }
                })
            })
            .collect()
    }
}

/// A driver holding nothing, behind [`Queue::inline`].
struct NoQueue;

#[async_trait]
impl QueueDriver for NoQueue {
    fn name(&self) -> &'static str {
        "inline"
    }
    async fn push(
        &self,
        _queue: &str,
        _name: &str,
        _payload: &serde_json::Value,
        _available_at: DateTime<Utc>,
        _unique_key: Option<&str>,
    ) -> CoreResult<Option<i64>> {
        Ok(None)
    }
    async fn claim(
        &self,
        _queues: &[String],
        _retry_after: Duration,
        _worker: &str,
    ) -> CoreResult<Option<Queued>> {
        Ok(None)
    }
    async fn release(&self, _id: i64, _available_at: DateTime<Utc>) -> CoreResult<()> {
        Ok(())
    }
    async fn delete(&self, _id: i64) -> CoreResult<()> {
        Ok(())
    }
    async fn fail(&self, _job: &Queued, _error: &str) -> CoreResult<i64> {
        Ok(0)
    }
    async fn counts(&self) -> CoreResult<Vec<QueueCount>> {
        Ok(Vec::new())
    }
    async fn failed(&self) -> CoreResult<Vec<FailedJob>> {
        Ok(Vec::new())
    }
    async fn retry(&self, _id: Option<i64>) -> CoreResult<usize> {
        Ok(0)
    }
    async fn forget(&self, _id: Option<i64>) -> CoreResult<usize> {
        Ok(0)
    }
    async fn clear(&self, _queue: Option<&str>) -> CoreResult<usize> {
        Ok(0)
    }
}

/// Announced when a job reaches the failed list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobFailed {
    pub id: i64,
    pub name: String,
    pub queue: String,
    pub attempts: u32,
    pub error: String,
}

impl Event for JobFailed {
    const NAME: &'static str = "queue.job_failed";
}

/// The queue a deployment configured: the database by default, Redis for
/// the `redis` store.
pub async fn from_config(
    config: &QueueConfig,
    jobs: impl IntoIterator<Item = JobReg>,
    services: Services,
) -> CoreResult<Queue> {
    let options = Options::from_config(config)?;
    match config.store.as_str() {
        "auto" | "database" => Ok(Queue::new(
            DatabaseQueue::new(services.db.clone()),
            jobs,
            services,
            options,
        )),
        #[cfg(feature = "redis")]
        "redis" => {
            let url = config.url.as_deref().ok_or_else(|| {
                CoreError::Config("queue.store = \"redis\" needs queue.url".to_string())
            })?;
            let driver = redis::connect(url, &config.prefix).await?;
            driver.ping().await?;
            Ok(Queue::new(driver, jobs, services, options))
        }
        #[cfg(not(feature = "redis"))]
        "redis" => Err(CoreError::Config(
            "queue.store = \"redis\" needs the `redis` feature of laterite-core".to_string(),
        )),
        other => Err(CoreError::Config(format!(
            "queue.store `{other}` is not one of auto, database, redis"
        ))),
    }
}

pub fn time_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_backoff_doubles_from_ten_seconds() {
        #[derive(Serialize, Deserialize)]
        struct Noop;
        #[async_trait]
        impl Job for Noop {
            const NAME: &'static str = "test::noop";
            async fn run(&self, _cx: &JobCx) -> Result<(), JobError> {
                Ok(())
            }
        }
        assert_eq!(Noop.backoff(1), Duration::from_secs(10));
        assert_eq!(Noop.backoff(2), Duration::from_secs(20));
        assert_eq!(Noop.backoff(4), Duration::from_secs(80));
        assert_eq!(Noop.backoff(40), Duration::from_secs(2560));
        assert_eq!(Noop.tries(), 3);
    }

    #[test]
    fn a_mode_parses_or_is_refused() {
        assert_eq!(Mode::parse("inline").unwrap(), Mode::Inline);
        assert!(Mode::parse("sometimes").is_err());
    }
}
