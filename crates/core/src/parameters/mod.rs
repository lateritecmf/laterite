//! The parameters store: state a module keeps for itself, keyed by name.
//!
//! A parameter is a JSON value under a `namespace::group.item` key: the
//! moment setup completed, the cursor of the last sync, a counter. It is
//! internal state, written by code, distinct from a settings model an
//! operator edits on a screen. The functions read and write the table
//! directly; [`Parameters`] wraps them with a per-process cache that a write
//! through it refreshes and a lifetime bounds.
//!
//! ```ignore
//! use laterite_core::parameters;
//!
//! parameters::set(&db, "acme.blog::sync.cursor", &cursor).await?;
//! let cursor: Option<String> = parameters::get(&db, "acme.blog::sync.cursor").await?;
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{CoreError, CoreResult};
use crate::query::{bind_values, build, AnyRowExt};
use crate::{Db, Module, ModuleId};
use sea_query::{Expr, Iden, OnConflict, Query};

pub mod migrations;

/// The parameters table.
#[derive(Iden)]
pub(crate) enum ParametersTable {
    #[iden = "parameters"]
    Table,
    Namespace,
    Group,
    Item,
    Value,
    UpdatedAt,
}

/// The module that owns the table, listed among an application's built-ins.
pub struct ParametersModule;

impl Module for ParametersModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> crate::MigrationSet {
        migrations::migrations()
    }
}

/// A key's three parts: `namespace::group.item`. The namespace is the owning
/// module's id; the item may itself contain dots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key<'a> {
    pub namespace: &'a str,
    pub group: &'a str,
    pub item: &'a str,
}

impl<'a> Key<'a> {
    /// Splits `namespace::group.item`; every part must be present and
    /// non-empty.
    pub fn parse(key: &'a str) -> CoreResult<Self> {
        let malformed = || CoreError::Validation(format!("malformed parameter key `{key}`"));
        let (namespace, rest) = key.split_once("::").ok_or_else(malformed)?;
        let (group, item) = rest.split_once('.').ok_or_else(malformed)?;
        if namespace.is_empty() || group.is_empty() || item.is_empty() {
            return Err(malformed());
        }
        Ok(Self {
            namespace,
            group,
            item,
        })
    }
}

/// Reads a parameter as `T`, or `None` when unset.
pub async fn get<T: DeserializeOwned>(db: &Db, key: &str) -> CoreResult<Option<T>> {
    match get_value(db, key).await? {
        Some(value) => Ok(Some(serde_json::from_value(value).map_err(|e| {
            CoreError::Internal(format!(
                "parameter `{key}` holds a value of another shape: {e}"
            ))
        })?)),
        None => Ok(None),
    }
}

/// Reads a parameter's raw JSON value, or `None` when unset.
pub async fn get_value(db: &Db, key: &str) -> CoreResult<Option<serde_json::Value>> {
    let parts = Key::parse(key)?;
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(ParametersTable::Value)
            .from(ParametersTable::Table)
            .and_where(Expr::col(ParametersTable::Namespace).eq(parts.namespace))
            .and_where(Expr::col(ParametersTable::Group).eq(parts.group))
            .and_where(Expr::col(ParametersTable::Item).eq(parts.item))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    match row {
        Some(r) => {
            let text = r.get_text("value")?;
            Ok(Some(serde_json::from_str(&text).map_err(|e| {
                CoreError::Internal(format!("parameter `{key}` holds invalid JSON: {e}"))
            })?))
        }
        None => Ok(None),
    }
}

/// Writes a parameter, replacing any value it held.
pub async fn set<T: Serialize + ?Sized>(db: &Db, key: &str, value: &T) -> CoreResult<()> {
    let parts = Key::parse(key)?;
    let json = serde_json::to_string(value)
        .map_err(|e| CoreError::Internal(format!("parameter `{key}` cannot serialize: {e}")))?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(ParametersTable::Table)
            .columns([
                ParametersTable::Namespace,
                ParametersTable::Group,
                ParametersTable::Item,
                ParametersTable::Value,
                ParametersTable::UpdatedAt,
            ])
            .values_panic([
                parts.namespace.into(),
                parts.group.into(),
                parts.item.into(),
                json.into(),
                now.into(),
            ])
            .on_conflict(
                OnConflict::columns([
                    ParametersTable::Namespace,
                    ParametersTable::Group,
                    ParametersTable::Item,
                ])
                .update_columns([ParametersTable::Value, ParametersTable::UpdatedAt])
                .to_owned(),
            )
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Removes a parameter. `true` when one was set.
pub async fn reset(db: &Db, key: &str) -> CoreResult<bool> {
    let parts = Key::parse(key)?;
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(ParametersTable::Table)
            .and_where(Expr::col(ParametersTable::Namespace).eq(parts.namespace))
            .and_where(Expr::col(ParametersTable::Group).eq(parts.group))
            .and_where(Expr::col(ParametersTable::Item).eq(parts.item))
            .to_owned(),
    );
    let done = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// What the cache holds for a key: the value (or its absence) and when it
/// was read.
#[derive(Debug, Clone)]
struct Cached {
    value: Option<serde_json::Value>,
    read_at: Instant,
}

/// The store behind a per-process cache. A read within `ttl` of the last
/// read or write of the same key returns the remembered value; a write or
/// reset through this handle refreshes it; another process's write is seen
/// once the entry ages out. Clones share the cache.
#[derive(Clone)]
pub struct Parameters {
    db: Db,
    cache: Arc<Mutex<HashMap<String, Cached>>>,
    ttl: Duration,
}

impl std::fmt::Debug for Parameters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parameters")
            .field("backend", &self.db.backend)
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

/// How long a cached read stays good: five minutes.
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

impl Parameters {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl: DEFAULT_TTL,
        }
    }

    /// The same store with another lifetime for cached reads.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// The connection the store writes through.
    pub fn db(&self) -> &Db {
        &self.db
    }

    fn remembered(&self, key: &str) -> Option<Option<serde_json::Value>> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .get(key)
            .filter(|c| c.read_at.elapsed() < self.ttl)
            .map(|c| c.value.clone())
    }

    fn remember(&self, key: &str, value: Option<serde_json::Value>) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(
            key.to_string(),
            Cached {
                value,
                read_at: Instant::now(),
            },
        );
    }

    /// Reads a parameter as `T`, from the cache when fresh.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> CoreResult<Option<T>> {
        let value = match self.remembered(key) {
            Some(value) => value,
            None => {
                let value = get_value(&self.db, key).await?;
                self.remember(key, value.clone());
                value
            }
        };
        match value {
            Some(value) => Ok(Some(serde_json::from_value(value).map_err(|e| {
                CoreError::Internal(format!(
                    "parameter `{key}` holds a value of another shape: {e}"
                ))
            })?)),
            None => Ok(None),
        }
    }

    /// Reads a parameter, or `default` when unset.
    pub async fn get_or<T: DeserializeOwned>(&self, key: &str, default: T) -> CoreResult<T> {
        Ok(self.get(key).await?.unwrap_or(default))
    }

    /// Writes a parameter and remembers it.
    pub async fn set<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> CoreResult<()> {
        set(&self.db, key, value).await?;
        let json = serde_json::to_value(value)
            .map_err(|e| CoreError::Internal(format!("parameter `{key}` cannot serialize: {e}")))?;
        self.remember(key, Some(json));
        Ok(())
    }

    /// Removes a parameter and remembers its absence.
    pub async fn reset(&self, key: &str) -> CoreResult<bool> {
        let done = reset(&self.db, key).await?;
        self.remember(key, None);
        Ok(done)
    }

    /// Drops one key from the cache, so the next read hits the table.
    pub fn forget(&self, key: &str) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.remove(key);
    }

    /// Drops every cached entry.
    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_splits_into_namespace_group_and_item() {
        let key = Key::parse("acme.blog::sync.cursor").unwrap();
        assert_eq!(key.namespace, "acme.blog");
        assert_eq!(key.group, "sync");
        assert_eq!(key.item, "cursor");
        let dotted = Key::parse("laterite.backend::setup.completed.at").unwrap();
        assert_eq!(dotted.group, "setup");
        assert_eq!(dotted.item, "completed.at");
    }

    #[test]
    fn a_malformed_key_is_refused() {
        for bad in [
            "",
            "sync.cursor",
            "acme::cursor",
            "::sync.cursor",
            "acme::.cursor",
            "acme::sync.",
        ] {
            assert!(
                matches!(Key::parse(bad), Err(CoreError::Validation(_))),
                "{bad}"
            );
        }
    }
}
