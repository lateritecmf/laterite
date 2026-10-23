//! The `memory` store: values in this process, each with its own lifetime,
//! the least recently used evicted when the capacity is reached.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use moka::future::Cache as Moka;
use moka::Expiry;

use super::{Entry, ValueStore};
use crate::error::CoreResult;

/// Sixty-four MiB, the capacity [`MemoryStore::new`] takes.
pub const DEFAULT_CAPACITY: u64 = 64 * 1024 * 1024;

struct Held {
    entry: Entry,
    ttl: Duration,
}

struct PerEntry;

impl Expiry<String, Arc<Held>> for PerEntry {
    fn expire_after_create(
        &self,
        _key: &String,
        held: &Arc<Held>,
        _at: Instant,
    ) -> Option<Duration> {
        Some(held.ttl)
    }
}

/// Values in this process's memory.
#[derive(Clone)]
pub struct MemoryStore {
    inner: Moka<String, Arc<Held>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    /// A store of [`DEFAULT_CAPACITY`] bytes.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// A store holding up to `bytes` of values (keys and overhead counted
    /// roughly).
    pub fn with_capacity(bytes: u64) -> Self {
        let inner = Moka::builder()
            .max_capacity(bytes.max(1))
            .weigher(|key: &String, held: &Arc<Held>| {
                let size = key.len() + held.entry.bytes.len() + 64;
                u32::try_from(size).unwrap_or(u32::MAX)
            })
            .expire_after(PerEntry)
            .build();
        Self { inner }
    }

    /// How many values are held (an estimate while writes are settling).
    pub fn len(&self) -> u64 {
        self.inner.entry_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl ValueStore for MemoryStore {
    fn name(&self) -> &'static str {
        "memory"
    }
    async fn get(&self, key: &str) -> CoreResult<Option<Entry>> {
        Ok(self.inner.get(key).await.map(|h| h.entry.clone()))
    }
    async fn put(&self, key: &str, entry: Entry, ttl: Duration) -> CoreResult<()> {
        self.inner
            .insert(key.to_string(), Arc::new(Held { entry, ttl }))
            .await;
        Ok(())
    }
    async fn add(&self, key: &str, entry: Entry, ttl: Duration) -> CoreResult<bool> {
        let mut stored = false;
        self.inner
            .entry(key.to_string())
            .or_insert_with(async {
                stored = true;
                Arc::new(Held { entry, ttl })
            })
            .await;
        Ok(stored)
    }
    async fn forget(&self, key: &str) -> CoreResult<bool> {
        Ok(self.inner.remove(key).await.is_some())
    }
    async fn flush(&self) -> CoreResult<()> {
        self.inner.invalidate_all();
        self.inner.run_pending_tasks().await;
        Ok(())
    }
}
