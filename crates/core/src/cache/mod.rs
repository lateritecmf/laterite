//! The cache: values kept for a time, invalidated by generation, shared by
//! every process of a deployment.
//!
//! A value goes in under a key with a lifetime and the names of the
//! namespaces it was built from; a read hands it back while every one of
//! those namespaces still carries the generation it had when the value was
//! stored. A write path bumps its namespace, and every value built from it
//! misses from then on, in this process at once and in the others within the
//! poll interval. Nothing is enumerated and nothing is listed by hand.
//!
//! ```ignore
//! let menu: Vec<Item> = ctx
//!     .cache()
//!     .remember("acme.blog::menu", Duration::from_secs(3600), &["records::posts"], || async {
//!         load_menu(ctx.db()).await
//!     })
//!     .await?;
//! ```
//!
//! Values live in a [`ValueStore`] (`memory`, `null` or `redis`) and the
//! generations and locks in a [`Signals`] store (the database, or Redis with
//! the `redis` store). [`from_config`] assembles the pair a deployment
//! configured; [`Cache::local`] is one process alone, for tests and tools.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::CacheConfig;
use crate::error::{CoreError, CoreResult};
use crate::{Db, Module, ModuleId};

mod memory;
pub mod migrations;
mod null;
#[cfg(feature = "redis")]
pub mod redis;
mod signals;

pub use memory::{MemoryStore, DEFAULT_CAPACITY};
pub use null::NullStore;
pub use signals::{DbSignals, LocalSignals};

/// The reserved namespace family of the record layer: `records::<entity>`,
/// bumped after every save and delete of that entity. Any module may depend
/// on one; none registers it.
pub fn records(entity: &str) -> String {
    format!("{RECORDS_PREFIX}{entity}")
}

const RECORDS_PREFIX: &str = "records::";

/// A namespace a module registers, `registry.add(CacheNamespace::new("feed"))`;
/// the framework qualifies it with the owning module's id, `acme.blog::feed`,
/// so one module cannot claim another's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheNamespace {
    name: &'static str,
}

impl CacheNamespace {
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }

    /// The name as registered, without the owner.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The full namespace, `owner::name`.
    pub fn qualified(&self, owner: ModuleId) -> String {
        format!("{}::{}", owner.as_str(), self.name)
    }
}

/// The module that owns the generation and lock tables, listed among an
/// application's built-ins.
pub struct CacheModule;

impl Module for CacheModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> crate::MigrationSet {
        migrations::migrations()
    }
}

/// A stored value: its bytes and the generation of each namespace it was
/// built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub bytes: Vec<u8>,
    pub deps: Vec<(String, u64)>,
}

/// Where values live. Every method answers for one key; a failure is the
/// store's own (a connection lost), never a miss.
#[async_trait]
pub trait ValueStore: Send + Sync {
    /// The driver's name, as `cache.store` names it.
    fn name(&self) -> &'static str;
    async fn get(&self, key: &str) -> CoreResult<Option<Entry>>;
    async fn put(&self, key: &str, entry: Entry, ttl: Duration) -> CoreResult<()>;
    /// Stores only when the key is absent; `true` when it did.
    async fn add(&self, key: &str, entry: Entry, ttl: Duration) -> CoreResult<bool>;
    /// `true` when a value was there.
    async fn forget(&self, key: &str) -> CoreResult<bool>;
    async fn flush(&self) -> CoreResult<()>;
}

/// Where generations and locks live, shared by every process.
///
/// A process keeps a replica of the generations: [`current`](Signals::current)
/// answers from it without waiting, a bump through this process updates it at
/// once, and [`refresh`](Signals::refresh) brings in the other processes'
/// bumps.
#[async_trait]
pub trait Signals: Send + Sync {
    fn name(&self) -> &'static str;
    /// The generation this process last saw, or `None` before the namespace
    /// was ever read here.
    fn current(&self, namespace: &str) -> Option<u64>;
    /// The generation, seeding the namespace when it is new.
    async fn ensure(&self, namespace: &str) -> CoreResult<u64>;
    /// Moves the namespace to a new generation and returns it.
    async fn bump(&self, namespace: &str) -> CoreResult<u64>;
    /// Reloads every generation from the shared store.
    async fn refresh(&self) -> CoreResult<()>;
    /// Every namespace the shared store knows, with its generation.
    async fn known(&self) -> CoreResult<Vec<(String, u64)>>;
    /// Takes the named lock for `owner` until `ttl` passes or it is released;
    /// `false` while another owner holds it.
    async fn lock(&self, name: &str, owner: &str, ttl: Duration) -> CoreResult<bool>;
    /// Releases the lock when `owner` holds it; `true` when it did.
    async fn unlock(&self, name: &str, owner: &str) -> CoreResult<bool>;
}

struct Inner {
    values: Box<dyn ValueStore>,
    signals: Box<dyn Signals>,
    /// The namespaces a value may depend on and a write may bump: those
    /// registered, plus the record family.
    allowed: RwLock<HashSet<String>>,
    /// One loader in flight per key: the second caller waits for the first
    /// and reads what it stored.
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// The cache handle. Cheap to clone: every clone is the same cache.
#[derive(Clone)]
pub struct Cache {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("store", &self.inner.values.name())
            .field("signals", &self.inner.signals.name())
            .finish_non_exhaustive()
    }
}

impl Cache {
    /// A cache over the given stores.
    pub fn new(values: impl ValueStore + 'static, signals: impl Signals + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                values: Box::new(values),
                signals: Box::new(signals),
                allowed: RwLock::new(HashSet::new()),
                flights: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// A cache for one process: values in memory, generations and locks
    /// in memory too, nothing shared. For tests, tools and an application
    /// assembled without the bootstrap.
    pub fn local() -> Self {
        Self::new(MemoryStore::new(), LocalSignals::new())
    }

    /// The configured value store's name.
    pub fn store(&self) -> &'static str {
        self.inner.values.name()
    }

    /// Registers a namespace, so values may depend on it and writes bump it.
    /// Registering one twice is nothing.
    pub fn allow(&self, namespace: impl Into<String>) {
        self.inner
            .allowed
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(namespace.into());
    }

    /// Whether the namespace is registered or in the record family.
    pub fn is_allowed(&self, namespace: &str) -> bool {
        namespace.starts_with(RECORDS_PREFIX)
            || self
                .inner
                .allowed
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(namespace)
    }

    /// The registered namespaces, sorted.
    pub fn namespaces(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .inner
            .allowed
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect();
        all.sort();
        all
    }

    fn check(&self, namespace: &str) -> CoreResult<()> {
        if self.is_allowed(namespace) {
            Ok(())
        } else {
            Err(CoreError::Validation(format!(
                "cache namespace `{namespace}` is not registered"
            )))
        }
    }

    /// The value under `key` while every namespace it depends on is at the
    /// generation it was stored at. A store failure reads as a miss and is
    /// logged.
    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        let entry = match self.inner.values.get(key).await {
            Ok(entry) => entry?,
            Err(e) => {
                tracing::warn!(key, error = %e, "cache read failed");
                return None;
            }
        };
        if self.fresh(&entry).await {
            Some(entry.bytes)
        } else {
            let _ = self.inner.values.forget(key).await;
            None
        }
    }

    async fn fresh(&self, entry: &Entry) -> bool {
        for (namespace, generation) in &entry.deps {
            let now = match self.inner.signals.current(namespace) {
                Some(now) => now,
                None => match self.inner.signals.ensure(namespace).await {
                    Ok(now) => now,
                    Err(e) => {
                        tracing::warn!(namespace, error = %e, "cache generation read failed");
                        return false;
                    }
                },
            };
            if now != *generation {
                return false;
            }
        }
        true
    }

    async fn deps_of(&self, deps: &[&str]) -> CoreResult<Vec<(String, u64)>> {
        let mut out = Vec::with_capacity(deps.len());
        for namespace in deps {
            self.check(namespace)?;
            out.push((namespace.to_string(), self.generation(namespace).await?));
        }
        Ok(out)
    }

    /// Stores `bytes` under `key` for `ttl`, built from `deps`. An
    /// unregistered namespace in `deps` is refused; a store failure is
    /// logged and the value is simply not kept.
    pub async fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        deps: &[&str],
        ttl: Duration,
    ) -> CoreResult<()> {
        let deps = self.deps_of(deps).await?;
        if let Err(e) = self.inner.values.put(key, Entry { bytes, deps }, ttl).await {
            tracing::warn!(key, error = %e, "cache write failed");
        }
        Ok(())
    }

    /// Like [`Cache::put`], only when nothing is under `key`; `true` when
    /// this call stored it.
    pub async fn add(
        &self,
        key: &str,
        bytes: Vec<u8>,
        deps: &[&str],
        ttl: Duration,
    ) -> CoreResult<bool> {
        let deps = self.deps_of(deps).await?;
        match self.inner.values.add(key, Entry { bytes, deps }, ttl).await {
            Ok(stored) => Ok(stored),
            Err(e) => {
                tracing::warn!(key, error = %e, "cache write failed");
                Ok(false)
            }
        }
    }

    /// Drops the value under `key`; `true` when one was there.
    pub async fn forget(&self, key: &str) -> bool {
        match self.inner.values.forget(key).await {
            Ok(had) => had,
            Err(e) => {
                tracing::warn!(key, error = %e, "cache forget failed");
                false
            }
        }
    }

    /// Drops every value this process's store holds. Other processes keep
    /// theirs; [`Cache::bump_all`] reaches them.
    pub async fn flush(&self) {
        if let Err(e) = self.inner.values.flush().await {
            tracing::warn!(error = %e, "cache flush failed");
        }
    }

    /// The JSON value under `key`, decoded as `T`.
    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let bytes = self.get(key).await?;
        match serde_json::from_slice(&bytes) {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(key, error = %e, "cached value has another shape");
                self.forget(key).await;
                None
            }
        }
    }

    /// Stores `value` as JSON under `key`.
    pub async fn put_json<T: Serialize + ?Sized>(
        &self,
        key: &str,
        value: &T,
        deps: &[&str],
        ttl: Duration,
    ) -> CoreResult<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| {
            CoreError::Internal(format!("cache value `{key}` cannot serialize: {e}"))
        })?;
        self.put(key, bytes, deps, ttl).await
    }

    /// The value under `key`, or what `load` returns, stored for `ttl` and
    /// built from `deps`. Concurrent callers for one key share one load.
    pub async fn remember<T, F, Fut>(
        &self,
        key: &str,
        ttl: Duration,
        deps: &[&str],
        load: F,
    ) -> CoreResult<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = CoreResult<T>>,
    {
        for namespace in deps {
            self.check(namespace)?;
        }
        if let Some(value) = self.get_json(key).await {
            return Ok(value);
        }
        let flight = {
            let mut flights = self.inner.flights.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(flights.entry(key.to_string()).or_default())
        };
        let outcome = {
            let _guard = flight.lock().await;
            match self.get_json(key).await {
                Some(value) => Ok(value),
                None => match load().await {
                    Ok(value) => {
                        self.put_json(key, &value, deps, ttl).await?;
                        Ok(value)
                    }
                    Err(e) => Err(e),
                },
            }
        };
        let mut flights = self.inner.flights.lock().unwrap_or_else(|e| e.into_inner());
        if flights.get(key).is_some_and(|f| Arc::strong_count(f) <= 2) {
            flights.remove(key);
        }
        outcome
    }

    /// The namespace's generation as this process last saw it, or `None`
    /// before it was ever read here.
    pub fn current(&self, namespace: &str) -> Option<u64> {
        self.inner.signals.current(namespace)
    }

    /// The namespace's generation, seeded when new.
    pub async fn generation(&self, namespace: &str) -> CoreResult<u64> {
        self.check(namespace)?;
        self.inner.signals.ensure(namespace).await
    }

    /// Moves the namespace on: every value built from it misses from now.
    /// Returns the new generation. A failure is logged as well as returned,
    /// since the write it follows already stands.
    pub async fn bump(&self, namespace: &str) -> CoreResult<u64> {
        self.check(namespace)?;
        match self.inner.signals.bump(namespace).await {
            Ok(generation) => {
                tracing::debug!(namespace, generation, "cache namespace bumped");
                Ok(generation)
            }
            Err(e) => {
                tracing::error!(namespace, error = %e, "cache bump failed");
                Err(e)
            }
        }
    }

    /// Bumps every namespace the shared store knows and drops this
    /// process's values. Returns how many namespaces moved.
    pub async fn bump_all(&self) -> CoreResult<usize> {
        let known = self.inner.signals.known().await?;
        for (namespace, _) in &known {
            self.inner.signals.bump(namespace).await?;
        }
        self.flush().await;
        Ok(known.len())
    }

    /// Every namespace the shared store knows, with its generation.
    pub async fn known(&self) -> CoreResult<Vec<(String, u64)>> {
        self.inner.signals.known().await
    }

    /// Brings in the other processes' bumps.
    pub async fn refresh(&self) -> CoreResult<()> {
        self.inner.signals.refresh().await
    }

    /// Refreshes every `every`, for the life of the process.
    pub fn start_polling(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let cache = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                if let Err(e) = cache.refresh().await {
                    tracing::warn!(error = %e, "cache generations refresh failed");
                }
            }
        })
    }

    /// Takes the named lock for `ttl`, or `None` while another holder has
    /// it. Release it with [`Lock::release`]; it lapses on its own after
    /// `ttl`.
    pub async fn lock(&self, name: &str, ttl: Duration) -> CoreResult<Option<Lock>> {
        let owner = format!("{:032x}", rand::random::<u128>());
        if self.inner.signals.lock(name, &owner, ttl).await? {
            Ok(Some(Lock {
                cache: self.clone(),
                name: name.to_string(),
                owner,
            }))
        } else {
            Ok(None)
        }
    }
}

/// A held lock. Release it when the work is done; dropped unreleased, it
/// lapses when its lifetime passes.
#[must_use = "a lock does nothing unless held for the work's duration"]
pub struct Lock {
    cache: Cache,
    name: String,
    owner: String,
}

impl Lock {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The holder's token, unique to this acquisition.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Lets the lock go; `true` when it was still held.
    pub async fn release(self) -> CoreResult<bool> {
        self.cache
            .inner
            .signals
            .unlock(&self.name, &self.owner)
            .await
    }
}

impl std::fmt::Debug for Lock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lock").field("name", &self.name).finish()
    }
}

/// The cache a deployment configured: values in the store `cache.store`
/// names (`auto` is `null` when `debug` and `memory` otherwise), generations
/// and locks in the database, or in Redis for the `redis` store.
pub async fn from_config(config: &CacheConfig, db: &Db, debug: bool) -> CoreResult<Cache> {
    let store = match config.store.as_str() {
        "auto" if debug => "null",
        "auto" => "memory",
        other => other,
    };
    match store {
        "null" => Ok(Cache::new(NullStore, DbSignals::new(db.clone()))),
        "memory" => Ok(Cache::new(
            MemoryStore::with_capacity(config.memory_mib.saturating_mul(1024 * 1024)),
            DbSignals::new(db.clone()),
        )),
        #[cfg(feature = "redis")]
        "redis" => {
            let url = config.url.as_deref().ok_or_else(|| {
                CoreError::Config("cache.store = \"redis\" needs cache.url".to_string())
            })?;
            let (values, signals) = redis::connect(url, &config.prefix).await?;
            Ok(Cache::new(values, signals))
        }
        #[cfg(not(feature = "redis"))]
        "redis" => Err(CoreError::Config(
            "cache.store = \"redis\" needs the `redis` feature of laterite-core".to_string(),
        )),
        other => Err(CoreError::Config(format!(
            "cache.store `{other}` is not one of auto, null, memory, redis"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_namespace_is_qualified_by_its_owner() {
        let ns = CacheNamespace::new("feed");
        assert_eq!(ns.qualified(ModuleId::new("acme.blog")), "acme.blog::feed");
        assert_eq!(records("posts"), "records::posts");
    }

    #[tokio::test]
    async fn an_unregistered_namespace_is_refused_and_records_are_not() {
        let cache = Cache::local();
        let err = cache
            .put("k", vec![1], &["acme.blog::feed"], Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Validation(_)));
        cache
            .put("k", vec![1], &[&records("posts")], Duration::from_secs(1))
            .await
            .unwrap();
        cache.allow("acme.blog::feed");
        cache
            .put("k2", vec![1], &["acme.blog::feed"], Duration::from_secs(1))
            .await
            .unwrap();
    }
}
