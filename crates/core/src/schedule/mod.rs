//! The scheduler: entries that run on a cron expression, evaluated every
//! minute in their own timezone, each on one process at a time.
//!
//! An entry runs a closure or pushes a job. A module registers its entries;
//! the ticker in the application process (or `schedule run` alone) checks
//! them each minute, takes a lock per entry and minute so one deployment
//! runs an entry once however many processes it has, skips an entry still
//! running from last time, and records each run for the screen.
//!
//! ```ignore
//! use laterite_core::schedule::{ScheduleEntry, ScheduleRegistry};
//!
//! fn register(&self, registry: &mut Registry) {
//!     registry.add_schedule(
//!         ScheduleEntry::new("acme.blog::digest", "0 7 * * *")
//!             .timezone("Europe/Berlin")
//!             .job(SendDigest { user_id: 0 }),
//!     );
//!     registry.add_schedule(
//!         ScheduleEntry::new("acme.blog::sweep", "*/5 * * * *")
//!             .run(|cx| async move { sweep(cx.db()).await }),
//!     );
//! }
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::cache::Cache;
use crate::config::ScheduleConfig;
use crate::error::{CoreError, CoreResult};
use crate::events::Events;
use crate::parameters::Parameters;
use crate::queue::{Job, Queue};
use crate::registry::Registry;
use crate::{Db, Module, ModuleId};
use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use croner::Cron;

pub mod migrations;
mod store;

pub use store::{EntryState, RunStatus};

/// The module that owns the `schedule_runs` table, listed among an
/// application's built-ins.
pub struct ScheduleModule;

impl Module for ScheduleModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> crate::MigrationSet {
        migrations::migrations()
    }
}

/// What a running entry sees.
#[derive(Clone)]
pub struct ScheduleCx {
    db: Db,
    events: Events,
    cache: Cache,
    parameters: Parameters,
    queue: Queue,
}

impl ScheduleCx {
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
    pub fn queue(&self) -> &Queue {
        &self.queue
    }
}

/// What happens when an entry is due while its last run is still going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overlap {
    /// Let this minute's run go: the entry runs once at a time.
    #[default]
    Skip,
    /// Run it beside the one still going.
    Allow,
}

type RunFn =
    dyn Fn(ScheduleCx) -> Pin<Box<dyn Future<Output = CoreResult<()>> + Send>> + Send + Sync;

#[derive(Clone)]
enum Action {
    Run(Arc<RunFn>),
    Job {
        name: &'static str,
        payload: serde_json::Value,
        queue: String,
    },
    /// Declared with neither `.run` nor `.job`; refused at boot.
    None,
}

/// One scheduled entry: a name, a cron expression, a timezone and what to do.
#[derive(Clone)]
pub struct ScheduleEntry {
    name: String,
    expression: String,
    timezone: String,
    description: String,
    action: Action,
    overlap: Overlap,
    one_server: bool,
}

impl std::fmt::Debug for ScheduleEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduleEntry")
            .field("name", &self.name)
            .field("expression", &self.expression)
            .field("timezone", &self.timezone)
            .finish_non_exhaustive()
    }
}

impl ScheduleEntry {
    /// An entry named `name` (`acme.blog::digest`) on `expression`: five
    /// fields (`0 7 * * *`), six with seconds, or a preset (`@hourly`,
    /// `@daily`, `@weekly`, `@monthly`, `@yearly`). A bad expression is
    /// refused when the application boots.
    pub fn new(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
            timezone: "UTC".to_string(),
            description: String::new(),
            action: Action::None,
            overlap: Overlap::Skip,
            one_server: true,
        }
    }

    /// Every `n` minutes.
    pub fn every_minutes(name: impl Into<String>, n: u32) -> Self {
        Self::new(name, format!("*/{} * * * *", n.max(1)))
    }

    /// Every hour, on the hour.
    pub fn hourly(name: impl Into<String>) -> Self {
        Self::new(name, "0 * * * *")
    }

    /// Every day at `HH:MM` in the entry's timezone.
    pub fn daily_at(name: impl Into<String>, time: &str) -> Self {
        let (hour, minute) = time.split_once(':').unwrap_or((time, "0"));
        let hour = hour.trim().parse::<u32>().unwrap_or(0);
        let minute = minute.trim().parse::<u32>().unwrap_or(0);
        Self::new(name, format!("{minute} {hour} * * *"))
    }

    /// The timezone the expression is read in; `UTC` unless set.
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = tz.into();
        self
    }

    /// A line for the screen.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Runs `f` when due.
    pub fn run<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(ScheduleCx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = CoreResult<()>> + Send + 'static,
    {
        self.action = Action::Run(Arc::new(move |cx| Box::pin(f(cx))));
        self
    }

    /// Pushes `job` when due.
    pub fn job<T: Job>(mut self, job: T) -> Self {
        let payload = serde_json::to_value(&job).unwrap_or(serde_json::Value::Null);
        self.action = Action::Job {
            name: T::NAME,
            payload,
            queue: job.queue(),
        };
        self
    }

    pub fn overlap(mut self, overlap: Overlap) -> Self {
        self.overlap = overlap;
        self
    }

    /// Whether one process of the deployment runs it per minute (`true`
    /// unless set) or every process does.
    pub fn one_server(mut self, one: bool) -> Self {
        self.one_server = one;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn expression(&self) -> &str {
        &self.expression
    }
}

/// `registry.add_schedule(entry)`: the typed way to contribute an entry.
pub trait ScheduleRegistry {
    fn add_schedule(&mut self, entry: ScheduleEntry);
}

impl ScheduleRegistry for Registry {
    fn add_schedule(&mut self, entry: ScheduleEntry) {
        self.add(entry);
    }
}

/// An entry parsed and ready.
struct Prepared {
    entry: ScheduleEntry,
    cron: Cron,
    tz: Tz,
}

impl Prepared {
    fn new(entry: ScheduleEntry) -> CoreResult<Self> {
        let cron: Cron = entry.expression.parse().map_err(|e| {
            CoreError::Config(format!(
                "schedule entry `{}`: expression `{}` does not parse: {e}",
                entry.name, entry.expression
            ))
        })?;
        let tz: Tz = entry.timezone.parse().map_err(|_| {
            CoreError::Config(format!(
                "schedule entry `{}`: timezone `{}` is unknown",
                entry.name, entry.timezone
            ))
        })?;
        if matches!(entry.action, Action::None) {
            return Err(CoreError::Config(format!(
                "schedule entry `{}` has nothing to do: give it `.run(..)` or `.job(..)`",
                entry.name
            )));
        }
        Ok(Self { entry, cron, tz })
    }

    fn is_due(&self, minute: DateTime<Utc>) -> bool {
        let local = minute.with_timezone(&self.tz);
        self.cron.is_time_matching(&local).unwrap_or(false)
    }

    fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let local = now.with_timezone(&self.tz);
        self.cron
            .find_next_occurrence(&local, false)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    }
}

struct Inner {
    entries: Vec<Prepared>,
    by_name: HashMap<String, usize>,
    db: Db,
    cx: ScheduleCx,
}

/// The schedule handle. Cheap to clone: every clone is the same schedule.
#[derive(Clone)]
pub struct Schedule {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Schedule")
            .field("entries", &self.inner.entries.len())
            .finish_non_exhaustive()
    }
}

/// The services an entry runs against.
#[derive(Clone)]
pub struct Services {
    pub db: Db,
    pub events: Events,
    pub cache: Cache,
    pub parameters: Parameters,
    pub queue: Queue,
}

/// How long the one-server lock on a minute is kept.
const MINUTE_LOCK: Duration = Duration::from_secs(3600);
/// How long an entry's overlap lock may be held by a run that never came back.
const OVERLAP_LOCK: Duration = Duration::from_secs(24 * 3600);

/// How one tick went: what ran, what was skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tick {
    pub ran: Vec<String>,
    pub skipped: Vec<String>,
}

impl Schedule {
    /// A schedule over `entries`; a bad expression, timezone or duplicate
    /// name is an error.
    pub fn new(
        entries: impl IntoIterator<Item = ScheduleEntry>,
        services: Services,
    ) -> CoreResult<Self> {
        let mut prepared = Vec::new();
        let mut by_name = HashMap::new();
        for entry in entries {
            if by_name.contains_key(&entry.name) {
                return Err(CoreError::Config(format!(
                    "schedule entry `{}` is registered twice",
                    entry.name
                )));
            }
            by_name.insert(entry.name.clone(), prepared.len());
            prepared.push(Prepared::new(entry)?);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                entries: prepared,
                by_name,
                db: services.db.clone(),
                cx: ScheduleCx {
                    db: services.db,
                    events: services.events,
                    cache: services.cache,
                    parameters: services.parameters,
                    queue: services.queue,
                },
            }),
        })
    }

    /// The entries' names, in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.inner
            .entries
            .iter()
            .map(|p| p.entry.name.as_str())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.entries.is_empty()
    }

    /// Every entry with its recorded state, in registration order.
    pub async fn list(&self) -> CoreResult<Vec<EntryState>> {
        let recorded = store::all(&self.inner.db).await?;
        let now = Utc::now();
        Ok(self
            .inner
            .entries
            .iter()
            .map(|p| {
                let mut state = recorded
                    .get(&p.entry.name)
                    .cloned()
                    .unwrap_or_else(|| EntryState::fresh(&p.entry.name));
                state.expression = p.entry.expression.clone();
                state.timezone = p.entry.timezone.clone();
                state.description = p.entry.description.clone();
                state.next_run_at = p.next_after(now).map(crate::queue::time_text);
                state
            })
            .collect())
    }

    /// Runs every entry due at `now`'s minute. Each runs on its own task;
    /// the tick returns once they are started, naming what ran and what was
    /// skipped (held by another process, or still running).
    pub async fn tick(&self, now: DateTime<Utc>) -> CoreResult<Tick> {
        let minute = now
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(now);
        let mut tick = Tick::default();
        for (at, prepared) in self.inner.entries.iter().enumerate() {
            if !prepared.is_due(minute) {
                continue;
            }
            match self.start(at, minute).await? {
                true => tick.ran.push(prepared.entry.name.clone()),
                false => tick.skipped.push(prepared.entry.name.clone()),
            }
        }
        Ok(tick)
    }

    /// Runs one entry now, due or not, under the same locks as a tick, and
    /// waits for it. `false` when it was skipped (still running elsewhere).
    pub async fn run_now(&self, name: &str) -> CoreResult<bool> {
        let Some(&at) = self.inner.by_name.get(name) else {
            return Err(CoreError::NotFound(format!("schedule entry `{name}`")));
        };
        match self.spawn(at, Utc::now()).await? {
            Some(handle) => {
                let _ = handle.await;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn start(&self, at: usize, minute: DateTime<Utc>) -> CoreResult<bool> {
        Ok(self.spawn(at, minute).await?.is_some())
    }

    /// Takes the locks and starts the entry on its own task; `None` when a
    /// lock was held elsewhere.
    async fn spawn(
        &self,
        at: usize,
        minute: DateTime<Utc>,
    ) -> CoreResult<Option<tokio::task::JoinHandle<()>>> {
        let prepared = &self.inner.entries[at];
        let name = prepared.entry.name.clone();
        let cache = &self.inner.cx.cache;
        if prepared.entry.one_server {
            let key = format!("laterite.schedule::{name}@{}", minute.format("%Y%m%d%H%M"));
            if cache.lock(&key, MINUTE_LOCK).await?.is_none() {
                tracing::debug!(entry = %name, "scheduled entry taken by another process");
                return Ok(None);
            }
        }
        let overlap = if prepared.entry.overlap == Overlap::Skip {
            match cache
                .lock(&format!("laterite.schedule::{name}"), OVERLAP_LOCK)
                .await?
            {
                Some(lock) => Some(lock),
                None => {
                    tracing::warn!(entry = %name, "scheduled entry still running; skipped");
                    store::skipped(&self.inner.db, &name).await?;
                    return Ok(None);
                }
            }
        } else {
            None
        };
        store::started(&self.inner.db, &name).await?;
        let schedule = self.clone();
        Ok(Some(tokio::spawn(async move {
            let outcome = schedule.execute(at).await;
            let db = &schedule.inner.db;
            match &outcome {
                Ok(()) => {
                    tracing::info!(entry = %name, "scheduled entry ran");
                    if let Err(e) = store::finished(db, &name, None).await {
                        tracing::error!(entry = %name, error = %e, "run could not be recorded");
                    }
                }
                Err(e) => {
                    tracing::error!(entry = %name, error = %e, "scheduled entry failed");
                    if let Err(e2) = store::finished(db, &name, Some(&e.to_string())).await {
                        tracing::error!(entry = %name, error = %e2, "failure could not be recorded");
                    }
                }
            }
            if let Some(lock) = overlap {
                let _ = lock.release().await;
            }
        })))
    }

    async fn execute(&self, at: usize) -> CoreResult<()> {
        let prepared = &self.inner.entries[at];
        match &prepared.entry.action {
            Action::Run(f) => f(self.inner.cx.clone()).await,
            Action::Job {
                name,
                payload,
                queue,
            } => self
                .inner
                .cx
                .queue
                .push_named(name, payload, queue)
                .await
                .map(|_| ()),
            Action::None => Ok(()),
        }
    }

    /// Ticks at every minute until `stop` says so.
    pub async fn run_ticker(&self, mut stop: tokio::sync::watch::Receiver<bool>) {
        loop {
            let now = Utc::now();
            let next_minute = (now + chrono::Duration::seconds(60))
                .with_second(0)
                .and_then(|t| t.with_nanosecond(0))
                .unwrap_or(now);
            let wait = (next_minute - now)
                .to_std()
                .unwrap_or(Duration::from_secs(1));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop.changed() => return,
            }
            if *stop.borrow() {
                return;
            }
            match self.tick(Utc::now()).await {
                Ok(tick) if !tick.ran.is_empty() || !tick.skipped.is_empty() => {
                    tracing::debug!(ran = ?tick.ran, skipped = ?tick.skipped, "schedule tick");
                }
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "schedule tick failed"),
            }
        }
    }

    /// Starts the ticker on its own task.
    pub fn spawn_ticker(
        &self,
        stop: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let schedule = self.clone();
        tokio::spawn(async move { schedule.run_ticker(stop).await })
    }
}

/// Every entry the table records, sorted by name: what a tool outside the
/// application can see of the schedule.
pub async fn recorded(db: &Db) -> CoreResult<Vec<EntryState>> {
    let mut rows: Vec<EntryState> = store::all(db).await?.into_values().collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// `ticker`, `external` or `off`, as `[schedule] mode` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ticker,
    External,
    Off,
}

impl Mode {
    pub fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "ticker" => Ok(Self::Ticker),
            "external" => Ok(Self::External),
            "off" => Ok(Self::Off),
            other => Err(CoreError::Config(format!(
                "schedule.mode `{other}` is not one of ticker, external, off"
            ))),
        }
    }
}

/// The mode `[schedule]` configures.
pub fn mode_from_config(config: &ScheduleConfig) -> CoreResult<Mode> {
    Mode::parse(&config.mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sugar_builds_the_expected_expressions() {
        assert_eq!(
            ScheduleEntry::every_minutes("a", 5).expression(),
            "*/5 * * * *"
        );
        assert_eq!(ScheduleEntry::hourly("a").expression(), "0 * * * *");
        assert_eq!(
            ScheduleEntry::daily_at("a", "07:30").expression(),
            "30 7 * * *"
        );
        assert_eq!(ScheduleEntry::daily_at("a", "7").expression(), "0 7 * * *");
    }

    #[test]
    fn an_entry_is_due_in_its_own_timezone() {
        let entry = ScheduleEntry::new("a", "0 7 * * *")
            .timezone("Asia/Colombo")
            .run(|_| async { Ok(()) });
        let prepared = Prepared::new(entry).unwrap();
        // 07:00 in Colombo is 01:30 UTC.
        let due = "2026-10-24T01:30:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(prepared.is_due(due));
        let not = "2026-10-24T07:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(!prepared.is_due(not));
        let next = prepared.next_after(not).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-10-25T01:30:00+00:00");
    }

    #[test]
    fn a_bad_expression_timezone_or_empty_entry_is_refused() {
        assert!(Prepared::new(ScheduleEntry::new("a", "nope").run(|_| async { Ok(()) })).is_err());
        assert!(Prepared::new(
            ScheduleEntry::hourly("a")
                .timezone("Mars/Olympus")
                .run(|_| async { Ok(()) })
        )
        .is_err());
        assert!(Prepared::new(ScheduleEntry::hourly("a")).is_err());
        assert_eq!(Mode::parse("off").unwrap(), Mode::Off);
    }
}
