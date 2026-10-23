//! The `redis` store: values, generations and locks in one Redis, shared
//! by every process. Keys carry the configured prefix.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;

use super::{Entry, Signals, ValueStore};
use crate::error::{CoreError, CoreResult};

fn io(e: redis::RedisError) -> CoreError {
    CoreError::Internal(format!("redis: {e}"))
}

/// Connects and returns the value store and the signals over one connection
/// manager, which reconnects on its own.
pub async fn connect(url: &str, prefix: &str) -> CoreResult<(RedisStore, RedisSignals)> {
    let client =
        redis::Client::open(url).map_err(|e| CoreError::Config(format!("cache.url: {e}")))?;
    let conn = client.get_connection_manager().await.map_err(io)?;
    let prefix = prefix.trim_end_matches(':').to_string();
    Ok((
        RedisStore {
            conn: conn.clone(),
            prefix: prefix.clone(),
        },
        RedisSignals {
            conn,
            prefix,
            replica: RwLock::new(HashMap::new()),
        },
    ))
}

const MAGIC: &[u8; 4] = b"LCE1";

/// `MAGIC`, the dependency list as JSON behind its length, then the bytes.
fn encode(entry: &Entry) -> Vec<u8> {
    let deps = serde_json::to_vec(&entry.deps).unwrap_or_default();
    let mut out = Vec::with_capacity(8 + deps.len() + entry.bytes.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(deps.len() as u32).to_be_bytes());
    out.extend_from_slice(&deps);
    out.extend_from_slice(&entry.bytes);
    out
}

fn decode(raw: &[u8]) -> Option<Entry> {
    let rest = raw.strip_prefix(MAGIC)?;
    let (len, rest) = rest.split_at_checked(4)?;
    let len = u32::from_be_bytes(len.try_into().ok()?) as usize;
    let (deps, bytes) = rest.split_at_checked(len)?;
    Some(Entry {
        deps: serde_json::from_slice(deps).ok()?,
        bytes: bytes.to_vec(),
    })
}

fn millis(ttl: Duration) -> u64 {
    u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1)
}

/// Values in Redis.
#[derive(Clone)]
pub struct RedisStore {
    conn: ConnectionManager,
    prefix: String,
}

impl RedisStore {
    fn key(&self, key: &str) -> String {
        format!("{}:v:{key}", self.prefix)
    }
}

#[async_trait]
impl ValueStore for RedisStore {
    fn name(&self) -> &'static str {
        "redis"
    }
    async fn get(&self, key: &str) -> CoreResult<Option<Entry>> {
        let mut conn = self.conn.clone();
        let raw: Option<Vec<u8>> = conn.get(self.key(key)).await.map_err(io)?;
        Ok(raw.as_deref().and_then(decode))
    }
    async fn put(&self, key: &str, entry: Entry, ttl: Duration) -> CoreResult<()> {
        let mut conn = self.conn.clone();
        let _: () = redis::cmd("SET")
            .arg(self.key(key))
            .arg(encode(&entry))
            .arg("PX")
            .arg(millis(ttl))
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        Ok(())
    }
    async fn add(&self, key: &str, entry: Entry, ttl: Duration) -> CoreResult<bool> {
        let mut conn = self.conn.clone();
        let set: Option<String> = redis::cmd("SET")
            .arg(self.key(key))
            .arg(encode(&entry))
            .arg("NX")
            .arg("PX")
            .arg(millis(ttl))
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        Ok(set.is_some())
    }
    async fn forget(&self, key: &str) -> CoreResult<bool> {
        let mut conn = self.conn.clone();
        let n: i64 = conn.del(self.key(key)).await.map_err(io)?;
        Ok(n > 0)
    }
    async fn flush(&self) -> CoreResult<()> {
        let mut conn = self.conn.clone();
        let pattern = format!("{}:v:*", self.prefix);
        let mut cursor: u64 = 0;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(500)
                .query_async(&mut conn)
                .await
                .map_err(io)?;
            if !keys.is_empty() {
                let _: i64 = conn.del(keys).await.map_err(io)?;
            }
            if next == 0 {
                return Ok(());
            }
            cursor = next;
        }
    }
}

/// Generations and locks in Redis, with a replica of the generations in
/// this process.
pub struct RedisSignals {
    conn: ConnectionManager,
    prefix: String,
    replica: RwLock<HashMap<String, u64>>,
}

impl RedisSignals {
    fn generation_key(&self, namespace: &str) -> String {
        format!("{}:g:{namespace}", self.prefix)
    }
    fn lock_key(&self, name: &str) -> String {
        format!("{}:l:{name}", self.prefix)
    }
    fn set_key(&self) -> String {
        format!("{}:namespaces", self.prefix)
    }
    fn remember(&self, namespace: &str, generation: u64) {
        self.replica
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(namespace.to_string(), generation);
    }
}

const RELEASE: &str =
    "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end";

#[async_trait]
impl Signals for RedisSignals {
    fn name(&self) -> &'static str {
        "redis"
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
        let mut conn = self.conn.clone();
        let key = self.generation_key(namespace);
        let seed = u64::from(rand::random::<u32>()) + 1;
        let _: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg(seed)
            .arg("NX")
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        let _: i64 = conn.sadd(self.set_key(), namespace).await.map_err(io)?;
        let generation: u64 = conn.get(&key).await.map_err(io)?;
        self.remember(namespace, generation);
        Ok(generation)
    }
    async fn bump(&self, namespace: &str) -> CoreResult<u64> {
        self.ensure(namespace).await?;
        let mut conn = self.conn.clone();
        let generation: u64 = conn
            .incr(self.generation_key(namespace), 1u64)
            .await
            .map_err(io)?;
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
        let mut conn = self.conn.clone();
        let mut names: Vec<String> = conn.smembers(self.set_key()).await.map_err(io)?;
        names.sort();
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<String> = names.iter().map(|n| self.generation_key(n)).collect();
        let values: Vec<Option<u64>> = conn.mget(keys).await.map_err(io)?;
        Ok(names
            .into_iter()
            .zip(values)
            .filter_map(|(n, g)| g.map(|g| (n, g)))
            .collect())
    }
    async fn lock(&self, name: &str, owner: &str, ttl: Duration) -> CoreResult<bool> {
        let mut conn = self.conn.clone();
        let set: Option<String> = redis::cmd("SET")
            .arg(self.lock_key(name))
            .arg(owner)
            .arg("NX")
            .arg("PX")
            .arg(millis(ttl))
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        Ok(set.is_some())
    }
    async fn unlock(&self, name: &str, owner: &str) -> CoreResult<bool> {
        let mut conn = self.conn.clone();
        let released: i64 = redis::Script::new(RELEASE)
            .key(self.lock_key(name))
            .arg(owner)
            .invoke_async(&mut conn)
            .await
            .map_err(io)?;
        Ok(released > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_round_trips_through_the_wire_format() {
        let entry = Entry {
            bytes: b"hello".to_vec(),
            deps: vec![("records::posts".into(), 7)],
        };
        assert_eq!(decode(&encode(&entry)), Some(entry));
        assert_eq!(decode(b"junk"), None);
    }
}
