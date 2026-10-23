//! The `database` driver: jobs in the `jobs` table, the failed list in
//! `failed_jobs`, claimed by one conditional update that only one worker
//! can win.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sea_query::{Expr, Iden, Order, Query};
use sqlx::Row;

use super::{time_text, FailedJob, QueueCount, QueueDriver, Queued};
use crate::error::{CoreError, CoreResult};
use crate::query::{bind_values, build, insert_returning_id, AnyRowExt};
use crate::Db;

#[derive(Iden)]
pub(crate) enum Jobs {
    Table,
    Id,
    Queue,
    Name,
    Payload,
    Attempts,
    AvailableAt,
    ReservedAt,
    ReservedBy,
    UniqueKey,
    CreatedAt,
}

#[derive(Iden)]
pub(crate) enum FailedJobs {
    Table,
    Id,
    Queue,
    Name,
    Payload,
    Error,
    FailedAt,
}

/// Jobs in the application's database.
#[derive(Clone)]
pub struct DatabaseQueue {
    db: Db,
}

impl DatabaseQueue {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    fn due(now: &str, stale_before: &str) -> sea_query::SimpleExpr {
        Expr::col(Jobs::ReservedAt)
            .is_null()
            .and(Expr::col(Jobs::AvailableAt).lte(now))
            .or(Expr::col(Jobs::ReservedAt).lte(stale_before))
    }

    async fn find(&self, id: i64) -> CoreResult<Option<Queued>> {
        let (sql, values) = build(
            self.db.backend,
            Query::select()
                .columns([
                    Jobs::Id,
                    Jobs::Queue,
                    Jobs::Name,
                    Jobs::Payload,
                    Jobs::Attempts,
                    Jobs::AvailableAt,
                    Jobs::ReservedAt,
                    Jobs::ReservedBy,
                    Jobs::CreatedAt,
                ])
                .from(Jobs::Table)
                .and_where(Expr::col(Jobs::Id).eq(id))
                .to_owned(),
        );
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(&self.db.pool)
            .await?;
        row.map(|r| queued_of(&r)).transpose()
    }
}

fn queued_of(r: &sqlx::any::AnyRow) -> CoreResult<Queued> {
    let payload = r.get_text("payload")?;
    Ok(Queued {
        id: r.try_get::<i64, _>("id")?,
        queue: r.get_text("queue")?,
        name: r.get_text("name")?,
        payload: serde_json::from_str(&payload)
            .map_err(|e| CoreError::Internal(format!("job payload is not JSON: {e}")))?,
        attempts: r.try_get::<i64, _>("attempts")?.max(0) as u32,
        available_at: r.get_text("available_at")?,
        reserved_at: r.get_text_opt("reserved_at")?,
        reserved_by: r.get_text_opt("reserved_by")?,
        created_at: r.get_text("created_at")?,
    })
}

#[async_trait]
impl QueueDriver for DatabaseQueue {
    fn name(&self) -> &'static str {
        "database"
    }

    async fn push(
        &self,
        queue: &str,
        name: &str,
        payload: &serde_json::Value,
        available_at: DateTime<Utc>,
        unique_key: Option<&str>,
    ) -> CoreResult<Option<i64>> {
        if let Some(key) = unique_key {
            let (sql, values) = build(
                self.db.backend,
                Query::select()
                    .expr(Expr::col(Jobs::Id).count())
                    .from(Jobs::Table)
                    .and_where(Expr::col(Jobs::UniqueKey).eq(key))
                    .to_owned(),
            );
            let row = bind_values(sqlx::query(&sql), values)
                .fetch_one(&self.db.pool)
                .await?;
            if row.try_get::<i64, _>(0)? > 0 {
                return Ok(None);
            }
        }
        let text = serde_json::to_string(payload)
            .map_err(|e| CoreError::Internal(format!("job payload cannot serialize: {e}")))?;
        let stmt = Query::insert()
            .into_table(Jobs::Table)
            .columns([
                Jobs::Queue,
                Jobs::Name,
                Jobs::Payload,
                Jobs::Attempts,
                Jobs::AvailableAt,
                Jobs::UniqueKey,
                Jobs::CreatedAt,
            ])
            .values_panic([
                queue.into(),
                name.into(),
                text.into(),
                0i64.into(),
                time_text(available_at).into(),
                unique_key.into(),
                time_text(Utc::now()).into(),
            ])
            .to_owned();
        let id = insert_returning_id(&self.db, stmt, Jobs::Id).await?;
        Ok(Some(id))
    }

    async fn claim(
        &self,
        queues: &[String],
        retry_after: Duration,
        worker: &str,
    ) -> CoreResult<Option<Queued>> {
        let stale = chrono::Duration::from_std(retry_after).unwrap_or(chrono::Duration::MAX);
        // Two workers may pick the same candidate; the conditional update
        // lets one of them have it and the other looks again.
        for _ in 0..5 {
            let now = time_text(Utc::now());
            let stale_before = time_text(Utc::now() - stale);
            let (sql, values) = build(
                self.db.backend,
                Query::select()
                    .column(Jobs::Id)
                    .from(Jobs::Table)
                    .and_where(Expr::col(Jobs::Queue).is_in(queues.iter().cloned()))
                    .and_where(Self::due(&now, &stale_before))
                    .order_by(Jobs::Id, Order::Asc)
                    .limit(1)
                    .to_owned(),
            );
            let row = bind_values(sqlx::query(&sql), values)
                .fetch_optional(&self.db.pool)
                .await?;
            let Some(row) = row else {
                return Ok(None);
            };
            let id = row.try_get::<i64, _>("id")?;
            let (sql, values) = build(
                self.db.backend,
                Query::update()
                    .table(Jobs::Table)
                    .value(Jobs::ReservedAt, now.clone())
                    .value(Jobs::ReservedBy, worker)
                    .value(Jobs::Attempts, Expr::col(Jobs::Attempts).add(1))
                    .and_where(Expr::col(Jobs::Id).eq(id))
                    .and_where(Self::due(&now, &stale_before))
                    .to_owned(),
            );
            let done = bind_values(sqlx::query(&sql), values)
                .execute(&self.db.pool)
                .await?;
            if done.rows_affected() == 1 {
                return self.find(id).await;
            }
        }
        Ok(None)
    }

    async fn release(&self, id: i64, available_at: DateTime<Utc>) -> CoreResult<()> {
        let (sql, values) = build(
            self.db.backend,
            Query::update()
                .table(Jobs::Table)
                .value(Jobs::ReservedAt, Option::<String>::None)
                .value(Jobs::ReservedBy, Option::<String>::None)
                .value(Jobs::AvailableAt, time_text(available_at))
                .and_where(Expr::col(Jobs::Id).eq(id))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    async fn delete(&self, id: i64) -> CoreResult<()> {
        let (sql, values) = build(
            self.db.backend,
            Query::delete()
                .from_table(Jobs::Table)
                .and_where(Expr::col(Jobs::Id).eq(id))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    async fn fail(&self, job: &Queued, error: &str) -> CoreResult<i64> {
        let text = serde_json::to_string(&job.payload)
            .map_err(|e| CoreError::Internal(format!("job payload cannot serialize: {e}")))?;
        let stmt = Query::insert()
            .into_table(FailedJobs::Table)
            .columns([
                FailedJobs::Queue,
                FailedJobs::Name,
                FailedJobs::Payload,
                FailedJobs::Error,
                FailedJobs::FailedAt,
            ])
            .values_panic([
                job.queue.as_str().into(),
                job.name.as_str().into(),
                text.into(),
                error.into(),
                time_text(Utc::now()).into(),
            ])
            .to_owned();
        let failed_id = insert_returning_id(&self.db, stmt, FailedJobs::Id).await?;
        self.delete(job.id).await?;
        Ok(failed_id)
    }

    async fn counts(&self) -> CoreResult<Vec<QueueCount>> {
        let (sql, values) = build(
            self.db.backend,
            Query::select()
                .columns([Jobs::Queue, Jobs::ReservedAt])
                .from(Jobs::Table)
                .to_owned(),
        );
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&self.db.pool)
            .await?;
        let mut counts: std::collections::BTreeMap<String, QueueCount> = Default::default();
        for r in rows {
            let queue = r.get_text("queue")?;
            let entry = counts.entry(queue.clone()).or_insert(QueueCount {
                queue,
                pending: 0,
                reserved: 0,
            });
            if r.get_text_opt("reserved_at")?.is_some() {
                entry.reserved += 1;
            } else {
                entry.pending += 1;
            }
        }
        Ok(counts.into_values().collect())
    }

    async fn failed(&self) -> CoreResult<Vec<FailedJob>> {
        let (sql, values) = build(
            self.db.backend,
            Query::select()
                .columns([
                    FailedJobs::Id,
                    FailedJobs::Queue,
                    FailedJobs::Name,
                    FailedJobs::Payload,
                    FailedJobs::Error,
                    FailedJobs::FailedAt,
                ])
                .from(FailedJobs::Table)
                .order_by(FailedJobs::Id, Order::Desc)
                .to_owned(),
        );
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&self.db.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let payload = r.get_text("payload")?;
            out.push(FailedJob {
                id: r.try_get::<i64, _>("id")?,
                queue: r.get_text("queue")?,
                name: r.get_text("name")?,
                payload: serde_json::from_str(&payload)
                    .map_err(|e| CoreError::Internal(format!("job payload is not JSON: {e}")))?,
                error: r.get_text("error")?,
                failed_at: r.get_text("failed_at")?,
            });
        }
        Ok(out)
    }

    async fn retry(&self, id: Option<i64>) -> CoreResult<usize> {
        let failed = self.failed().await?;
        let mut moved = 0;
        for job in failed
            .into_iter()
            .filter(|f| id.is_none_or(|id| f.id == id))
        {
            self.push(&job.queue, &job.name, &job.payload, Utc::now(), None)
                .await?;
            self.forget(Some(job.id)).await?;
            moved += 1;
        }
        Ok(moved)
    }

    async fn forget(&self, id: Option<i64>) -> CoreResult<usize> {
        let (sql, values) = {
            let mut stmt = Query::delete();
            stmt.from_table(FailedJobs::Table);
            if let Some(id) = id {
                stmt.and_where(Expr::col(FailedJobs::Id).eq(id));
            }
            build(self.db.backend, stmt)
        };
        let done = bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        Ok(done.rows_affected() as usize)
    }

    async fn clear(&self, queue: Option<&str>) -> CoreResult<usize> {
        let (sql, values) = {
            let mut stmt = Query::delete();
            stmt.from_table(Jobs::Table)
                .and_where(Expr::col(Jobs::ReservedAt).is_null());
            if let Some(queue) = queue {
                stmt.and_where(Expr::col(Jobs::Queue).eq(queue));
            }
            build(self.db.backend, stmt)
        };
        let done = bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        Ok(done.rows_affected() as usize)
    }
}
