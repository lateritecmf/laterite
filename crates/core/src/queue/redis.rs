//! The `redis` driver: jobs in Redis, shared by every process, claimed by
//! one script so two workers never take the same job.
//!
//! Keys under the prefix: `ids` (the counter), `queues` (every queue seen),
//! `job:<id>` (a hash), `waiting:<queue>` and `reserved:<queue>` (sorted
//! sets by due time and claim time), `failed` and `failed:<id>`, `unique`.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;

use super::{time_text, FailedJob, QueueCount, QueueDriver, Queued};
use crate::error::{CoreError, CoreResult};

fn io(e: redis::RedisError) -> CoreError {
    CoreError::Internal(format!("redis: {e}"))
}

fn epoch(at: DateTime<Utc>) -> f64 {
    at.timestamp_millis() as f64 / 1000.0
}

/// Connects; the manager reconnects on its own.
pub async fn connect(url: &str, prefix: &str) -> CoreResult<RedisQueue> {
    let client =
        redis::Client::open(url).map_err(|e| CoreError::Config(format!("queue.url: {e}")))?;
    let conn = client.get_connection_manager().await.map_err(io)?;
    Ok(RedisQueue {
        conn,
        prefix: prefix.trim_end_matches(':').to_string(),
    })
}

/// Jobs in Redis.
#[derive(Clone)]
pub struct RedisQueue {
    conn: ConnectionManager,
    prefix: String,
}

/// Reclaims claims older than ARGV[1], then takes the oldest job due by
/// ARGV[2] for worker ARGV[3] at ARGV[4]. KEYS: waiting, reserved, prefix.
const CLAIM: &str = r#"
local stale = redis.call('ZRANGEBYSCORE', KEYS[2], '-inf', ARGV[1])
for _, id in ipairs(stale) do
  redis.call('ZREM', KEYS[2], id)
  redis.call('ZADD', KEYS[1], '0', id)
  redis.call('HDEL', KEYS[3] .. ':job:' .. id, 'reserved_at', 'reserved_by')
end
local due = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[2], 'LIMIT', '0', '1')
if #due == 0 then return false end
local id = due[1]
redis.call('ZREM', KEYS[1], id)
redis.call('ZADD', KEYS[2], ARGV[2], id)
redis.call('HINCRBY', KEYS[3] .. ':job:' .. id, 'attempts', '1')
redis.call('HSET', KEYS[3] .. ':job:' .. id, 'reserved_at', ARGV[4], 'reserved_by', ARGV[3])
return id
"#;

impl RedisQueue {
    fn key(&self, rest: &str) -> String {
        format!("{}:{rest}", self.prefix)
    }

    async fn read(&self, id: i64) -> CoreResult<Option<Queued>> {
        let mut conn = self.conn.clone();
        let fields: std::collections::HashMap<String, String> = conn
            .hgetall(self.key(&format!("job:{id}")))
            .await
            .map_err(io)?;
        if fields.is_empty() {
            return Ok(None);
        }
        let get = |k: &str| fields.get(k).cloned().unwrap_or_default();
        Ok(Some(Queued {
            id,
            queue: get("queue"),
            name: get("name"),
            payload: serde_json::from_str(&get("payload"))
                .map_err(|e| CoreError::Internal(format!("job payload is not JSON: {e}")))?,
            attempts: get("attempts").parse().unwrap_or(0),
            available_at: get("available_at"),
            reserved_at: fields.get("reserved_at").cloned(),
            reserved_by: fields.get("reserved_by").cloned(),
            created_at: get("created_at"),
        }))
    }

    async fn remove(&self, job: &Queued) -> CoreResult<()> {
        let mut conn = self.conn.clone();
        // The unique key is read before the hash goes, so it is freed.
        let unique: Option<String> = conn
            .hget(self.key(&format!("job:{}", job.id)), "unique_key")
            .await
            .map_err(io)?;
        let _: () = redis::pipe()
            .zrem(self.key(&format!("waiting:{}", job.queue)), job.id)
            .zrem(self.key(&format!("reserved:{}", job.queue)), job.id)
            .del(self.key(&format!("job:{}", job.id)))
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        if let Some(unique) = unique {
            let _: i64 = conn.srem(self.key("unique"), unique).await.map_err(io)?;
        }
        Ok(())
    }

    async fn queues(&self) -> CoreResult<Vec<String>> {
        let mut conn = self.conn.clone();
        let mut queues: Vec<String> = conn.smembers(self.key("queues")).await.map_err(io)?;
        queues.sort();
        Ok(queues)
    }
}

#[async_trait]
impl QueueDriver for RedisQueue {
    fn name(&self) -> &'static str {
        "redis"
    }

    async fn push(
        &self,
        queue: &str,
        name: &str,
        payload: &serde_json::Value,
        available_at: DateTime<Utc>,
        unique_key: Option<&str>,
    ) -> CoreResult<Option<i64>> {
        let mut conn = self.conn.clone();
        if let Some(key) = unique_key {
            let added: i64 = conn.sadd(self.key("unique"), key).await.map_err(io)?;
            if added == 0 {
                return Ok(None);
            }
        }
        let id: i64 = conn.incr(self.key("ids"), 1i64).await.map_err(io)?;
        let text = serde_json::to_string(payload)
            .map_err(|e| CoreError::Internal(format!("job payload cannot serialize: {e}")))?;
        let mut fields: Vec<(&str, String)> = vec![
            ("queue", queue.to_string()),
            ("name", name.to_string()),
            ("payload", text),
            ("attempts", "0".to_string()),
            ("available_at", time_text(available_at)),
            ("created_at", time_text(Utc::now())),
        ];
        if let Some(key) = unique_key {
            fields.push(("unique_key", key.to_string()));
        }
        let _: () = redis::pipe()
            .hset_multiple(self.key(&format!("job:{id}")), &fields)
            .zadd(
                self.key(&format!("waiting:{queue}")),
                id,
                epoch(available_at),
            )
            .sadd(self.key("queues"), queue)
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        Ok(Some(id))
    }

    async fn claim(
        &self,
        queues: &[String],
        retry_after: Duration,
        worker: &str,
    ) -> CoreResult<Option<Queued>> {
        let mut conn = self.conn.clone();
        let now = Utc::now();
        let stale_before =
            epoch(now - chrono::Duration::from_std(retry_after).unwrap_or(chrono::Duration::MAX));
        let script = redis::Script::new(CLAIM);
        for queue in queues {
            let id: Option<i64> = script
                .key(self.key(&format!("waiting:{queue}")))
                .key(self.key(&format!("reserved:{queue}")))
                .key(&self.prefix)
                .arg(stale_before)
                .arg(epoch(now))
                .arg(worker)
                .arg(time_text(now))
                .invoke_async(&mut conn)
                .await
                .map_err(io)?;
            if let Some(id) = id {
                return self.read(id).await;
            }
        }
        Ok(None)
    }

    async fn release(&self, id: i64, available_at: DateTime<Utc>) -> CoreResult<()> {
        let Some(job) = self.read(id).await? else {
            return Ok(());
        };
        let mut conn = self.conn.clone();
        let _: () = redis::pipe()
            .zrem(self.key(&format!("reserved:{}", job.queue)), id)
            .zadd(
                self.key(&format!("waiting:{}", job.queue)),
                id,
                epoch(available_at),
            )
            .hset(
                self.key(&format!("job:{id}")),
                "available_at",
                time_text(available_at),
            )
            .hdel(
                self.key(&format!("job:{id}")),
                &["reserved_at", "reserved_by"],
            )
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        Ok(())
    }

    async fn delete(&self, id: i64) -> CoreResult<()> {
        if let Some(job) = self.read(id).await? {
            self.remove(&job).await?;
        }
        Ok(())
    }

    async fn fail(&self, job: &Queued, error: &str) -> CoreResult<i64> {
        let mut conn = self.conn.clone();
        let now = Utc::now();
        let text = serde_json::to_string(&job.payload)
            .map_err(|e| CoreError::Internal(format!("job payload cannot serialize: {e}")))?;
        let fields: Vec<(&str, String)> = vec![
            ("queue", job.queue.clone()),
            ("name", job.name.clone()),
            ("payload", text),
            ("error", error.to_string()),
            ("failed_at", time_text(now)),
        ];
        let _: () = redis::pipe()
            .hset_multiple(self.key(&format!("failed:{}", job.id)), &fields)
            .zadd(self.key("failed"), job.id, epoch(now))
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        self.remove(job).await?;
        Ok(job.id)
    }

    async fn counts(&self) -> CoreResult<Vec<QueueCount>> {
        let mut conn = self.conn.clone();
        let mut out = Vec::new();
        for queue in self.queues().await? {
            let pending: u64 = conn
                .zcard(self.key(&format!("waiting:{queue}")))
                .await
                .map_err(io)?;
            let reserved: u64 = conn
                .zcard(self.key(&format!("reserved:{queue}")))
                .await
                .map_err(io)?;
            if pending + reserved > 0 {
                out.push(QueueCount {
                    queue,
                    pending,
                    reserved,
                });
            }
        }
        Ok(out)
    }

    async fn failed(&self) -> CoreResult<Vec<FailedJob>> {
        let mut conn = self.conn.clone();
        let ids: Vec<i64> = conn
            .zrevrange(self.key("failed"), 0, -1)
            .await
            .map_err(io)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let fields: std::collections::HashMap<String, String> = conn
                .hgetall(self.key(&format!("failed:{id}")))
                .await
                .map_err(io)?;
            if fields.is_empty() {
                continue;
            }
            let get = |k: &str| fields.get(k).cloned().unwrap_or_default();
            out.push(FailedJob {
                id,
                queue: get("queue"),
                name: get("name"),
                payload: serde_json::from_str(&get("payload")).unwrap_or(serde_json::Value::Null),
                error: get("error"),
                failed_at: get("failed_at"),
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
        let mut conn = self.conn.clone();
        let ids: Vec<i64> = match id {
            Some(id) => vec![id],
            None => conn.zrange(self.key("failed"), 0, -1).await.map_err(io)?,
        };
        let mut gone = 0;
        for id in ids {
            let removed: i64 = conn.zrem(self.key("failed"), id).await.map_err(io)?;
            let _: i64 = conn
                .del(self.key(&format!("failed:{id}")))
                .await
                .map_err(io)?;
            gone += removed as usize;
        }
        Ok(gone)
    }

    async fn clear(&self, queue: Option<&str>) -> CoreResult<usize> {
        let queues = match queue {
            Some(q) => vec![q.to_string()],
            None => self.queues().await?,
        };
        let mut conn = self.conn.clone();
        let mut gone = 0;
        for queue in queues {
            let ids: Vec<i64> = conn
                .zrange(self.key(&format!("waiting:{queue}")), 0, -1)
                .await
                .map_err(io)?;
            for id in ids {
                if let Some(job) = self.read(id).await? {
                    self.remove(&job).await?;
                    gone += 1;
                }
            }
        }
        Ok(gone)
    }

    async fn ping(&self) -> CoreResult<()> {
        let mut conn = self.conn.clone();
        let _: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(io)?;
        Ok(())
    }
}
