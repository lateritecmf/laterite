//! The request log: every URL the admin could not serve, counted, so a broken
//! link or a probing client shows up as a row rather than a line in a file.
//!
//! The 404 fallback records the method, the path and the status, once per
//! distinct triple with a count and the first and last times; the log is a
//! read-only resource an operator may clear row by row, and it keeps at most
//! [`CAP`] rows, dropping the least recently seen beyond that.

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
}

/// Rows kept; beyond it the least recently seen go.
pub(crate) const CAP: i64 = 2000;
/// How many go at once when the cap is passed, so the trim is rare.
const TRIM: u32 = 200;
/// The longest path recorded; a longer one is cut, since it is caller-chosen.
const MAX_PATH: usize = 512;

/// Counts one unserved request. Never fails the response: an error is logged.
pub(crate) async fn record(db: &Db, method: &str, path: &str, status: u16) {
    if let Err(e) = record_in(db, method, path, status).await {
        tracing::warn!(error = %e, "recording an unserved request failed");
    }
}

async fn record_in(db: &Db, method: &str, path: &str, status: u16) -> Result<(), sqlx::Error> {
    let path: String = path.chars().take(MAX_PATH).collect();
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = db.pool.begin().await?;
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([BackendRequestLog::Id, BackendRequestLog::Count])
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
            let (sql, values) = build(
                db.backend,
                Query::update()
                    .table(BackendRequestLog::Table)
                    .value(BackendRequestLog::Count, count + 1)
                    .value(BackendRequestLog::LastSeen, now)
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
                ])
                .values_panic([
                    method.into(),
                    path.into(),
                    i32::from(status).into(),
                    1i64.into(),
                    now.clone().into(),
                    now.into(),
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
