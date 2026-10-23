//! Generations and locks: in this process alone ([`LocalSignals`]) or in
//! the database, shared by every process ([`DbSignals`]).

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use sea_query::{Expr, Iden, Query};

use super::Signals;
use crate::error::CoreResult;
use crate::query::{bind_values, build, on_conflict_ignore, AnyRowExt};
use crate::Db;
use sqlx::Row;

/// A fresh generation: random and never zero, so a namespace seeded again
/// after a reset never repeats a value a store still holds.
fn seed() -> u64 {
    u64::from(rand::random::<u32>()) + 1
}

/// Generations and locks held by this process alone.
#[derive(Default)]
pub struct LocalSignals {
    generations: RwLock<HashMap<String, u64>>,
    locks: Mutex<HashMap<String, (String, Instant)>>,
}

impl LocalSignals {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Signals for LocalSignals {
    fn name(&self) -> &'static str {
        "local"
    }
    fn current(&self, namespace: &str) -> Option<u64> {
        self.generations
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(namespace)
            .copied()
    }
    async fn ensure(&self, namespace: &str) -> CoreResult<u64> {
        let mut all = self.generations.write().unwrap_or_else(|e| e.into_inner());
        Ok(*all.entry(namespace.to_string()).or_insert_with(seed))
    }
    async fn bump(&self, namespace: &str) -> CoreResult<u64> {
        let mut all = self.generations.write().unwrap_or_else(|e| e.into_inner());
        let next = all.entry(namespace.to_string()).or_insert_with(seed);
        *next += 1;
        Ok(*next)
    }
    async fn refresh(&self) -> CoreResult<()> {
        Ok(())
    }
    async fn known(&self) -> CoreResult<Vec<(String, u64)>> {
        let all = self.generations.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(String, u64)> = all.iter().map(|(k, v)| (k.clone(), *v)).collect();
        out.sort();
        Ok(out)
    }
    async fn lock(&self, name: &str, owner: &str, ttl: Duration) -> CoreResult<bool> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if locks.get(name).is_some_and(|(_, until)| *until > now) {
            return Ok(false);
        }
        locks.insert(name.to_string(), (owner.to_string(), now + ttl));
        Ok(true)
    }
    async fn unlock(&self, name: &str, owner: &str) -> CoreResult<bool> {
        let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        if locks.get(name).is_some_and(|(held, _)| held == owner) {
            locks.remove(name);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[derive(Iden)]
pub(crate) enum CacheGenerations {
    #[iden = "cache_generations"]
    Table,
    Namespace,
    Generation,
}

#[derive(Iden)]
pub(crate) enum CacheLocks {
    #[iden = "cache_locks"]
    Table,
    Name,
    Owner,
    ExpiresAt,
}

fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn until_text(ttl: Duration) -> String {
    let ttl = chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
    (Utc::now() + ttl).to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Generations and locks in the database, with a replica of the generations
/// in this process.
pub struct DbSignals {
    db: Db,
    replica: RwLock<HashMap<String, u64>>,
}

impl DbSignals {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            replica: RwLock::new(HashMap::new()),
        }
    }

    fn remember(&self, namespace: &str, generation: u64) {
        self.replica
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(namespace.to_string(), generation);
    }

    async fn read(&self, namespace: &str) -> CoreResult<Option<u64>> {
        let (sql, values) = build(
            self.db.backend,
            Query::select()
                .column(CacheGenerations::Generation)
                .from(CacheGenerations::Table)
                .and_where(Expr::col(CacheGenerations::Namespace).eq(namespace))
                .to_owned(),
        );
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(&self.db.pool)
            .await?;
        Ok(match row {
            Some(r) => Some(r.try_get::<i64, _>("generation")?.max(0) as u64),
            None => None,
        })
    }

    async fn insert_if_new(&self, namespace: &str) -> CoreResult<()> {
        let (sql, values) = build(
            self.db.backend,
            Query::insert()
                .into_table(CacheGenerations::Table)
                .columns([CacheGenerations::Namespace, CacheGenerations::Generation])
                .values_panic([namespace.into(), (seed() as i64).into()])
                .on_conflict(on_conflict_ignore([CacheGenerations::Namespace]))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Signals for DbSignals {
    fn name(&self) -> &'static str {
        "database"
    }
    fn current(&self, namespace: &str) -> Option<u64> {
        self.replica
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(namespace)
            .copied()
    }
    async fn ensure(&self, namespace: &str) -> CoreResult<u64> {
        if let Some(current) = self.current(namespace) {
            return Ok(current);
        }
        let generation = match self.read(namespace).await? {
            Some(generation) => generation,
            None => {
                self.insert_if_new(namespace).await?;
                self.read(namespace).await?.unwrap_or_else(seed)
            }
        };
        self.remember(namespace, generation);
        Ok(generation)
    }
    async fn bump(&self, namespace: &str) -> CoreResult<u64> {
        self.insert_if_new(namespace).await?;
        let (sql, values) = build(
            self.db.backend,
            Query::update()
                .table(CacheGenerations::Table)
                .value(
                    CacheGenerations::Generation,
                    Expr::col(CacheGenerations::Generation).add(1),
                )
                .and_where(Expr::col(CacheGenerations::Namespace).eq(namespace))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        let generation = self.read(namespace).await?.unwrap_or_else(seed);
        self.remember(namespace, generation);
        Ok(generation)
    }
    async fn refresh(&self) -> CoreResult<()> {
        let known = self.known().await?;
        let mut replica = self.replica.write().unwrap_or_else(|e| e.into_inner());
        for (namespace, generation) in known {
            replica.insert(namespace, generation);
        }
        Ok(())
    }
    async fn known(&self) -> CoreResult<Vec<(String, u64)>> {
        let (sql, values) = build(
            self.db.backend,
            Query::select()
                .columns([CacheGenerations::Namespace, CacheGenerations::Generation])
                .from(CacheGenerations::Table)
                .order_by(CacheGenerations::Namespace, sea_query::Order::Asc)
                .to_owned(),
        );
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&self.db.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push((
                r.get_text("namespace")?,
                r.try_get::<i64, _>("generation")?.max(0) as u64,
            ));
        }
        Ok(out)
    }
    async fn lock(&self, name: &str, owner: &str, ttl: Duration) -> CoreResult<bool> {
        // A lapsed lock goes first, then the insert either lands or finds
        // the row held; the read says whose it is.
        let (sql, values) = build(
            self.db.backend,
            Query::delete()
                .from_table(CacheLocks::Table)
                .and_where(Expr::col(CacheLocks::Name).eq(name))
                .and_where(Expr::col(CacheLocks::ExpiresAt).lt(now_text()))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        let (sql, values) = build(
            self.db.backend,
            Query::insert()
                .into_table(CacheLocks::Table)
                .columns([CacheLocks::Name, CacheLocks::Owner, CacheLocks::ExpiresAt])
                .values_panic([name.into(), owner.into(), until_text(ttl).into()])
                .on_conflict(on_conflict_ignore([CacheLocks::Name]))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        let (sql, values) = build(
            self.db.backend,
            Query::select()
                .column(CacheLocks::Owner)
                .from(CacheLocks::Table)
                .and_where(Expr::col(CacheLocks::Name).eq(name))
                .to_owned(),
        );
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(&self.db.pool)
            .await?;
        Ok(match row {
            Some(r) => r.get_text("owner")? == owner,
            None => false,
        })
    }
    async fn unlock(&self, name: &str, owner: &str) -> CoreResult<bool> {
        let (sql, values) = build(
            self.db.backend,
            Query::delete()
                .from_table(CacheLocks::Table)
                .and_where(Expr::col(CacheLocks::Name).eq(name))
                .and_where(Expr::col(CacheLocks::Owner).eq(owner))
                .to_owned(),
        );
        let done = bind_values(sqlx::query(&sql), values)
            .execute(&self.db.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
