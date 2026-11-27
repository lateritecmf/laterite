//! The request log: every URL the admin could not serve, counted, so a broken
//! link or a probing client shows up as a row rather than a line in a file.
//!
//! The 404 fallback records the method, the path and the status, once per
//! distinct triple with a count and the first and last times; the log is a
//! read-only resource an operator may clear row by row, and it keeps at most
//! 2000 rows, dropping the least recently seen beyond that.

mod migrations;

use laterite_core::query::{bind_values, build};
use laterite_core::strata::Iden;
use laterite_core::{AnyRowExt, Db, MigrationSet, Module, ModuleId};
use sea_query::{Expr, Order, Query};

/// The log's module: its table migrates with the admin's.
pub struct RequestLogModule;

#[laterite_core::strata::async_trait]
impl Module for RequestLogModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }
    /// Trims the log to its cap, so a table that grew under an older cap
    /// starts within bounds.
    async fn boot(&self, cx: &laterite_core::BootCx<'_>) -> laterite_core::CoreResult<()> {
        let mut tx = cx.db.pool.begin().await?;
        trim(cx.db, &mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[derive(Iden)]
pub(crate) enum BackendRequestLog {
    Table,
    Id,
    Method,
    Path,
    Status,
    Count,
    FirstSeen,
    LastSeen,
    Referrers,
}

/// Rows kept; beyond it the least recently seen go.
pub(crate) const CAP: i64 = 2000;
/// How many go at once when the cap is passed, so the trim is rare.
const TRIM: u32 = 200;
/// The longest path recorded; a longer one is cut, since it is caller-chosen.
const MAX_PATH: usize = 512;

/// The most referrers kept for one row.
const MAX_REFERRERS: usize = 10;

/// What the Log settings screen stores.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LogSetting {
    /// Whether warnings and errors are kept in the event log.
    pub log_events: bool,
    /// Whether unserved URLs are kept in the request log.
    pub log_requests: bool,
}

impl Default for LogSetting {
    fn default() -> Self {
        Self {
            log_events: true,
            log_requests: true,
        }
    }
}

impl crate::settings::SettingsModel for LogSetting {
    const CODE: &'static str = "laterite.log";
}

impl LogSetting {
    /// What is stored, or both logs on when it cannot be read.
    pub(crate) async fn current(db: &Db) -> Self {
        crate::settings::load::<Self>(db).await.unwrap_or_default()
    }
}

/// The Log settings screen.
pub(crate) fn settings_item() -> crate::settings::SettingsItem {
    use crate::form::FormField;
    crate::settings::SettingsItem::new(
        <LogSetting as crate::settings::SettingsModel>::CODE,
        laterite_core::t!("Log Settings"),
        vec![
            FormField::switch("log_events", laterite_core::t!("Keep the event log"))
                .default_value(true)
                .help(laterite_core::t!(
                    "Warnings and errors the application logs are stored for the Event Log."
                )),
            FormField::switch("log_requests", laterite_core::t!("Keep the request log"))
                .default_value(true)
                .help(laterite_core::t!(
                    "URLs that could not be served are counted for the Request Log."
                )),
        ],
    )
    .description(laterite_core::t!("Choose which logs are kept."))
    .category(crate::settings::category::LOGS)
    .slot(laterite_core::arrange::slot::SYSTEM)
    .order(33)
    .icon("sliders-horizontal")
    .permission("backend.manage_settings")
}

/// Counts one unserved request. Never fails the response: an error is logged.
pub(crate) async fn record(db: &Db, method: &str, path: &str, status: u16) {
    record_from(db, method, path, status, None).await;
}

/// [`record`], with the page that linked here when the request named one.
/// Nothing is kept while the Log settings turn the request log off.
pub(crate) async fn record_from(
    db: &Db,
    method: &str,
    path: &str,
    status: u16,
    referrer: Option<&str>,
) {
    if !LogSetting::current(db).await.log_requests {
        return;
    }
    if let Err(e) = record_in(db, method, path, status, referrer).await {
        tracing::warn!(error = %e, "recording an unserved request failed");
    }
}

/// `kept` with `referrer` added, unless it is there or the list is full.
fn with_referrer(kept: Option<String>, referrer: Option<&str>) -> Option<String> {
    let Some(referrer) = referrer.map(str::trim).filter(|r| !r.is_empty()) else {
        return kept;
    };
    let referrer: String = referrer
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_PATH)
        .collect();
    let mut lines: Vec<String> = kept
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    if lines.len() < MAX_REFERRERS && !lines.contains(&referrer) {
        lines.push(referrer);
    }
    Some(lines.join("\n"))
}

async fn record_in(
    db: &Db,
    method: &str,
    path: &str,
    status: u16,
    referrer: Option<&str>,
) -> Result<(), sqlx::Error> {
    let path: String = path.chars().take(MAX_PATH).collect();
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = db.pool.begin().await?;
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                BackendRequestLog::Id,
                BackendRequestLog::Count,
                BackendRequestLog::Referrers,
            ])
            .from(BackendRequestLog::Table)
            .and_where(Expr::col(BackendRequestLog::Method).eq(method))
            .and_where(Expr::col(BackendRequestLog::Path).eq(path.clone()))
            .and_where(Expr::col(BackendRequestLog::Status).eq(i32::from(status)))
            .to_owned(),
    );
    let seen = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&mut *tx)
        .await?;
    match seen {
        Some(row) => {
            let id: i64 = row.get_int("id").unwrap_or_default();
            let count: i64 = row.get_int("count").unwrap_or_default();
            let kept: Option<String> = row.get_text_opt("referrers").ok().flatten();
            let referrers = with_referrer(kept, referrer);
            let (sql, values) = build(
                db.backend,
                Query::update()
                    .table(BackendRequestLog::Table)
                    .value(BackendRequestLog::Count, count + 1)
                    .value(BackendRequestLog::LastSeen, now)
                    .value(BackendRequestLog::Referrers, referrers)
                    .and_where(Expr::col(BackendRequestLog::Id).eq(id))
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .execute(&mut *tx)
                .await?;
        }
        None => {
            let insert = Query::insert()
                .into_table(BackendRequestLog::Table)
                .columns([
                    BackendRequestLog::Method,
                    BackendRequestLog::Path,
                    BackendRequestLog::Status,
                    BackendRequestLog::Count,
                    BackendRequestLog::FirstSeen,
                    BackendRequestLog::LastSeen,
                    BackendRequestLog::Referrers,
                ])
                .values_panic([
                    method.into(),
                    path.into(),
                    i32::from(status).into(),
                    1i64.into(),
                    now.clone().into(),
                    now.into(),
                    with_referrer(None, referrer).into(),
                ])
                .to_owned();
            let (sql, values) = build(db.backend, insert);
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
            .expr(Expr::col(BackendRequestLog::Id).count())
            .from(BackendRequestLog::Table)
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
            .column(BackendRequestLog::Id)
            .from(BackendRequestLog::Table)
            .order_by(BackendRequestLog::LastSeen, Order::Asc)
            .limit(u64::from(TRIM))
            .to_owned(),
    );
    let oldest: Vec<i64> = bind_values(sqlx::query(&sql), values)
        .fetch_all(&mut **tx)
        .await?
        .iter()
        .filter_map(|r| r.get_int("id").ok())
        .collect();
    if oldest.is_empty() {
        return Ok(());
    }
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendRequestLog::Table)
            .and_where(Expr::col(BackendRequestLog::Id).is_in(oldest))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
