//! The `schedule_runs` table: one row per entry that has run or been
//! skipped, holding its last run for the screen.

use std::collections::HashMap;

use chrono::Utc;
use sea_query::{Expr, Iden, Query};
use serde::{Deserialize, Serialize};

use crate::error::CoreResult;
use crate::query::{bind_values, build, on_conflict_ignore, AnyRowExt};
use crate::queue::time_text;
use crate::Db;

#[derive(Iden)]
pub(crate) enum ScheduleRuns {
    Table,
    Name,
    LastStartedAt,
    LastFinishedAt,
    LastStatus,
    LastError,
}

/// How an entry's last run ended.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Never run on this deployment.
    Never,
    Running,
    Ok,
    Failed,
    /// Due while the last run was still going.
    Skipped,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "ok" => Self::Ok,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            _ => Self::Never,
        }
    }
}

/// One entry as the screen shows it: its declaration and its last run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntryState {
    pub name: String,
    pub expression: String,
    pub timezone: String,
    pub description: String,
    pub next_run_at: Option<String>,
    pub last_started_at: Option<String>,
    pub last_finished_at: Option<String>,
    pub last_status: RunStatus,
    pub last_error: Option<String>,
}

impl EntryState {
    pub(crate) fn fresh(name: &str) -> Self {
        Self {
            name: name.to_string(),
            expression: String::new(),
            timezone: String::new(),
            description: String::new(),
            next_run_at: None,
            last_started_at: None,
            last_finished_at: None,
            last_status: RunStatus::Never,
            last_error: None,
        }
    }
}

pub(crate) async fn all(db: &Db) -> CoreResult<HashMap<String, EntryState>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                ScheduleRuns::Name,
                ScheduleRuns::LastStartedAt,
                ScheduleRuns::LastFinishedAt,
                ScheduleRuns::LastStatus,
                ScheduleRuns::LastError,
            ])
            .from(ScheduleRuns::Table)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut out = HashMap::with_capacity(rows.len());
    for r in rows {
        let name = r.get_text("name")?;
        out.insert(
            name.clone(),
            EntryState {
                last_started_at: r.get_text_opt("last_started_at")?,
                last_finished_at: r.get_text_opt("last_finished_at")?,
                last_status: RunStatus::parse(&r.get_text("last_status")?),
                last_error: r.get_text_opt("last_error")?,
                ..EntryState::fresh(&name)
            },
        );
    }
    Ok(out)
}

async fn ensure(db: &Db, name: &str) -> CoreResult<()> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(ScheduleRuns::Table)
            .columns([ScheduleRuns::Name, ScheduleRuns::LastStatus])
            .values_panic([name.into(), RunStatus::Never.as_str().into()])
            .on_conflict(on_conflict_ignore([ScheduleRuns::Name]))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub(crate) async fn started(db: &Db, name: &str) -> CoreResult<()> {
    ensure(db, name).await?;
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(ScheduleRuns::Table)
            .value(ScheduleRuns::LastStartedAt, time_text(Utc::now()))
            .value(ScheduleRuns::LastFinishedAt, Option::<String>::None)
            .value(ScheduleRuns::LastStatus, RunStatus::Running.as_str())
            .value(ScheduleRuns::LastError, Option::<String>::None)
            .and_where(Expr::col(ScheduleRuns::Name).eq(name))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub(crate) async fn finished(db: &Db, name: &str, error: Option<&str>) -> CoreResult<()> {
    let status = if error.is_some() {
        RunStatus::Failed
    } else {
        RunStatus::Ok
    };
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(ScheduleRuns::Table)
            .value(ScheduleRuns::LastFinishedAt, time_text(Utc::now()))
            .value(ScheduleRuns::LastStatus, status.as_str())
            .value(ScheduleRuns::LastError, error.map(str::to_string))
            .and_where(Expr::col(ScheduleRuns::Name).eq(name))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub(crate) async fn skipped(db: &Db, name: &str) -> CoreResult<()> {
    ensure(db, name).await?;
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(ScheduleRuns::Table)
            .value(ScheduleRuns::LastStatus, RunStatus::Skipped.as_str())
            .and_where(Expr::col(ScheduleRuns::Name).eq(name))
            .and_where(Expr::col(ScheduleRuns::LastStatus).ne(RunStatus::Running.as_str()))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}
