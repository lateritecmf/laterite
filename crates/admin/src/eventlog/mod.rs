//! The event log: the warnings and errors the application logs, kept as rows
//! an operator can read, so finding out why something failed does not need
//! access to the server.
//!
//! [`layer`] is a `tracing` layer that hands every warning and error to a
//! writer task. The writer stores each distinct message once, with a count
//! and the first and last times, and the log keeps at most [`CAP`] rows,
//! dropping the least recently seen beyond that. The framework installs the
//! layer with its own log output; an application that installs a subscriber
//! of its own adds [`layer`] to it.
//!
//! Nothing the log does can fail a request: a full queue drops the event, and
//! a failed write goes to standard error.

mod migrations;

use std::sync::OnceLock;

use laterite_core::query::{bind_values, build};
use laterite_core::strata::Iden;
use laterite_core::{AnyRowExt, Db, MigrationSet, Module, ModuleId};
use sea_query::{Expr, Order, Query};
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// The log's module: its table migrates with the admin's, and its writer
/// starts once the database is there.
pub struct EventLogModule;

#[laterite_core::strata::async_trait]
impl Module for EventLogModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }
    async fn boot(&self, cx: &laterite_core::BootCx<'_>) -> laterite_core::CoreResult<()> {
        start(cx.db.clone());
        Ok(())
    }
}

#[derive(Iden)]
pub(crate) enum BackendEventLog {
    Table,
    Id,
    Level,
    Target,
    Message,
    Detail,
    Count,
    FirstSeen,
    LastSeen,
}

/// Rows kept; beyond it the least recently seen go.
pub const CAP: i64 = 2000;
/// How many go at once when the cap is passed, so the trim is rare.
const TRIM: u64 = 200;
/// Events waiting to be written; one arriving at a full queue is dropped.
const QUEUE: usize = 256;
/// The longest message and detail stored.
const MAX_MESSAGE: usize = 2000;
const MAX_DETAIL: usize = 8000;

/// One logged warning or error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `error` or `warn`.
    pub level: &'static str,
    /// The module that logged it (`acme::orders`).
    pub target: String,
    /// What it said.
    pub message: String,
    /// Its other fields, one a line, then where in the source it was logged.
    pub detail: String,
}

static QUEUED: OnceLock<mpsc::Sender<Entry>> = OnceLock::new();

/// The `tracing` layer that feeds the log. It passes on warnings and errors
/// and nothing below them, and nothing the database layer or the log itself
/// says, so writing an entry can never cause another.
pub fn layer() -> EventLogLayer {
    EventLogLayer
}

/// The layer [`layer`] returns.
pub struct EventLogLayer;

#[derive(Default)]
struct Fields {
    message: String,
    rest: Vec<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.rest.push(format!("{}: {value}", field.name()));
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.rest.push(format!("{}: {value:?}", field.name()));
        }
    }
}

/// Whether the log leaves `target` out: the database layer's own chatter and
/// this module's, either of which would feed the log from its own writes.
fn skipped(target: &str) -> bool {
    target.starts_with("sqlx") || target.starts_with("laterite_admin::eventlog")
}

/// The entry for an event at `level` from `target`, or `None` for one the log
/// does not keep.
fn entry(level: &Level, target: &str, fields: Fields, at: Option<(&str, u32)>) -> Option<Entry> {
    let level = match *level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        _ => return None,
    };
    if skipped(target) {
        return None;
    }
    let mut detail = fields.rest.join("\n");
    if let Some((file, line)) = at {
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&format!("at {file}:{line}"));
    }
    let message = if fields.message.is_empty() {
        // An event of fields alone is named by them.
        fields.rest.first().cloned().unwrap_or_default()
    } else {
        fields.message
    };
    Some(Entry {
        level,
        target: target.to_string(),
        message: message.chars().take(MAX_MESSAGE).collect(),
        detail: detail.chars().take(MAX_DETAIL).collect(),
    })
}

impl<S: Subscriber> Layer<S> for EventLogLayer {
    fn on_event(&self, event: &Event<'_>, _cx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() > Level::WARN {
            return;
        }
        let Some(queue) = QUEUED.get() else {
            return;
        };
        let mut fields = Fields::default();
        event.record(&mut fields);
        let at = meta.file().zip(meta.line());
        if let Some(entry) = entry(meta.level(), meta.target(), fields, at) {
            // A full queue means the log is behind: this one is let go.
            let _ = queue.try_send(entry);
        }
    }
}

/// Starts the writer for `db`. The first database to start it keeps it: a
/// process has one log.
pub(crate) fn start(db: Db) {
    let (tx, mut rx) = mpsc::channel::<Entry>(QUEUE);
    if QUEUED.set(tx).is_err() {
        return;
    }
    tokio::spawn(async move {
        while let Some(entry) = rx.recv().await {
            if let Err(error) = record(&db, &entry).await {
                eprintln!("event log: could not store an entry: {error}");
            }
        }
    });
}

/// Stores one entry: a message already there from the same place at the same
/// level is counted again, a new one is added and the log trimmed to its cap.
pub async fn record(db: &Db, entry: &Entry) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = db.pool.begin().await?;
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([BackendEventLog::Id, BackendEventLog::Count])
            .from(BackendEventLog::Table)
            .and_where(Expr::col(BackendEventLog::Level).eq(entry.level))
            .and_where(Expr::col(BackendEventLog::Target).eq(entry.target.clone()))
            .and_where(Expr::col(BackendEventLog::Message).eq(entry.message.clone()))
            .limit(1)
            .to_owned(),
    );
    let seen = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&mut *tx)
        .await?;
    match seen {
        Some(row) => {
            let id: i64 = row.get_int("id").unwrap_or_default();
            let count: i64 = row.get_int("count").unwrap_or_default();
            let (sql, values) = build(
                db.backend,
                Query::update()
                    .table(BackendEventLog::Table)
                    .value(BackendEventLog::Count, count + 1)
                    .value(BackendEventLog::Detail, entry.detail.clone())
                    .value(BackendEventLog::LastSeen, now)
                    .and_where(Expr::col(BackendEventLog::Id).eq(id))
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .execute(&mut *tx)
                .await?;
        }
        None => {
            let (sql, values) = build(
                db.backend,
                Query::insert()
                    .into_table(BackendEventLog::Table)
                    .columns([
                        BackendEventLog::Level,
                        BackendEventLog::Target,
                        BackendEventLog::Message,
                        BackendEventLog::Detail,
                        BackendEventLog::Count,
                        BackendEventLog::FirstSeen,
                        BackendEventLog::LastSeen,
                    ])
                    .values_panic([
                        entry.level.into(),
                        entry.target.clone().into(),
                        entry.message.clone().into(),
                        entry.detail.clone().into(),
                        1i64.into(),
                        now.clone().into(),
                        now.into(),
                    ])
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .execute(&mut *tx)
                .await?;
            trim(db, &mut tx).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Drops the least recently seen rows once the cap is passed. The ids are
/// read first and deleted by list, which every backend accepts.
async fn trim(db: &Db, tx: &mut sqlx::Transaction<'_, sqlx::Any>) -> Result<(), sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr(Expr::col(BackendEventLog::Id).count())
            .from(BackendEventLog::Table)
            .to_owned(),
    );
    let (total,): (i64,) = laterite_core::query::bind_values_as(sqlx::query_as(&sql), values)
        .fetch_one(&mut **tx)
        .await?;
    if total <= CAP {
        return Ok(());
    }
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(BackendEventLog::Id)
            .from(BackendEventLog::Table)
            .order_by(BackendEventLog::LastSeen, Order::Asc)
            .limit(TRIM)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&mut **tx)
        .await?;
    let ids: Vec<i64> = rows
        .iter()
        .filter_map(|row| row.get_int("id").ok())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendEventLog::Table)
            .and_where(Expr::col(BackendEventLog::Id).is_in(ids))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(message: &str, rest: &[&str]) -> Fields {
        Fields {
            message: message.to_string(),
            rest: rest.iter().map(|r| String::from(*r)).collect(),
        }
    }

    #[test]
    fn warnings_and_errors_are_kept_and_nothing_below() {
        let kept = entry(
            &Level::ERROR,
            "acme::orders",
            fields("could not charge", &["order: 42"]),
            Some(("src/orders.rs", 17)),
        )
        .unwrap();
        assert_eq!(kept.level, "error");
        assert_eq!(kept.message, "could not charge");
        assert_eq!(kept.detail, "order: 42\nat src/orders.rs:17");
        assert!(entry(&Level::INFO, "acme::orders", fields("fine", &[]), None).is_none());
        assert_eq!(
            entry(&Level::WARN, "acme", fields("slow", &[]), None)
                .unwrap()
                .level,
            "warn"
        );
    }

    #[test]
    fn the_database_layer_and_the_log_itself_are_left_out() {
        for target in ["sqlx::query", "laterite_admin::eventlog"] {
            assert!(entry(&Level::ERROR, target, fields("x", &[]), None).is_none());
        }
    }

    #[test]
    fn an_event_of_fields_alone_is_named_by_its_first() {
        let kept = entry(
            &Level::WARN,
            "acme",
            fields("", &["error: timed out"]),
            None,
        )
        .unwrap();
        assert_eq!(kept.message, "error: timed out");
    }

    #[tokio::test]
    async fn a_repeated_message_is_one_row_counted() {
        let (db, _guard) = laterite_core::testing::connect_test(&[migrations::migrations()]).await;
        let first = Entry {
            level: "error",
            target: "acme::orders".into(),
            message: "could not charge".into(),
            detail: "order: 42".into(),
        };
        record(&db, &first).await.unwrap();
        record(
            &db,
            &Entry {
                detail: "order: 43".into(),
                ..first.clone()
            },
        )
        .await
        .unwrap();
        record(
            &db,
            &Entry {
                level: "warn",
                ..first.clone()
            },
        )
        .await
        .unwrap();
        let (sql, values) = build(
            db.backend,
            Query::select()
                .columns([
                    BackendEventLog::Level,
                    BackendEventLog::Count,
                    BackendEventLog::Detail,
                ])
                .from(BackendEventLog::Table)
                .order_by(BackendEventLog::Id, Order::Asc)
                .to_owned(),
        );
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "the same message at another level is its own row"
        );
        assert_eq!(rows[0].get_int("count").unwrap(), 2);
        assert_eq!(
            rows[0].get_text("detail").unwrap(),
            "order: 43",
            "the latest detail is kept"
        );
        assert_eq!(rows[1].get_text("level").unwrap(), "warn");
    }
}
