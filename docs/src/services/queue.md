# Queue

A job is a struct that implements `Job`; pushing it stores the struct as JSON, and a worker runs it with retries, a timeout and a failed list.

```rust
use laterite_core::queue::{Job, JobCx, JobError, QueueRegistry};
use laterite_core::strata::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SendDigest {
    user_id: i64,
}

#[async_trait]
impl Job for SendDigest {
    const NAME: &'static str = "acme.blog::send_digest";

    async fn run(&self, cx: &JobCx) -> Result<(), JobError> {
        digest::send(cx.db(), self.user_id).await?;
        Ok(())
    }
}

fn register(&self, registry: &mut Registry) {
    registry.add_job::<SendDigest>();
}
```

```rust
ctx.queue().push(&SendDigest { user_id: 7 }).await?;
ctx.queue().push_in(&SendDigest { user_id: 7 }, Duration::from_secs(600)).await?;
ctx.queue().push_at(&SendDigest { user_id: 7 }, tonight).await?;
```

Method | Does
--- | ---
`push(&job)` | Stores the job, due now. `Pushed::Queued(id)`, `Pushed::Ran` in inline mode, `Pushed::Duplicate` when its unique key is already waiting.
`push_in(&job, delay)` | Due after `delay`.
`push_at(&job, when)` | Due at `when`.
`counts()` | How many jobs wait and run on each queue.
`failed()` | The failed list, newest first.
`retry(id)` / `forget(id)` | Put a failed job back, or drop it; `None` for every one.
`clear(queue)` | Drops the waiting jobs on one queue, or on all with `None`.

A job whose type is not registered is refused at push with `CoreError::Validation`.

## Set the job's rules

```rust
#[async_trait]
impl Job for SendDigest {
    const NAME: &'static str = "acme.blog::send_digest";
    async fn run(&self, cx: &JobCx) -> Result<(), JobError> { /* ... */ }

    fn queue(&self) -> String { "mail".into() }
    fn tries(&self) -> u32 { 5 }
    fn backoff(&self, attempt: u32) -> Duration { Duration::from_secs(30 * u64::from(attempt)) }
    fn timeout(&self) -> Option<Duration> { Some(Duration::from_secs(120)) }
    fn retry_until(&self) -> Option<DateTime<Utc>> { Some(Utc::now() + Duration::hours(6)) }
    fn unique_key(&self) -> Option<String> { Some(format!("digest:{}", self.user_id)) }
}
```

Method | Default | Meaning
--- | --- | ---
`queue()` | `default` | The queue the job lands on.
`tries()` | `3` | Attempts before the job fails; `0` leaves it to `retry_until`.
`backoff(attempt)` | 10 s, doubling | The wait after attempt `attempt` failed.
`timeout()` | 60 s | The longest one attempt may run; `None` for no limit.
`retry_until()` | `None` | Keep retrying until this moment, whatever `tries` says.
`unique_key()` | `None` | A push while a job with this key waits is dropped.

Return | Effect
--- | ---
`Ok(())` | The job is removed.
`Err(JobError::retry(message))` | Released for the next attempt after the backoff, or failed on the last one.
`Err(JobError::fail(message))` | Failed at once, whatever tries remain.

`JobCx` gives a job `db()`, `events()`, `cache()`, `parameters()`, its `attempt()` and `queue()`.

## Configure the queue

```toml
# config/default.toml
[queue]
store = "auto"            # auto | database | redis
mode = "worker"           # worker | external | inline
workers = 2
queues = ["default"]
retry_after_seconds = 90
poll_seconds = 1
```

Key | Description
--- | ---
`queue.store` | Where jobs wait. `auto` is `database`.
`queue.mode` | `worker` runs the workers in the application process; `external` leaves them to `queue work`; `inline` runs a job as it is pushed.
`queue.workers` | How many workers the process runs in `worker` mode. Default `2`.
`queue.queues` | The queues a worker takes from, in priority order. Default `["default"]`.
`queue.retry_after_seconds` | After this long a claimed job counts as abandoned and is claimed again. Longer than any job's timeout. Default `90`.
`queue.poll_seconds` | How long an idle worker waits before looking again. Default `1`.

## Run a worker apart

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bootstrap::new("config")
        .module(migrations::AppModule)
        .run()
        .await
}
```

```bash
lat queue work                       # cargo run -- queue work
lat queue work --once
lat queue work --stop-when-empty --queue high,default
acme queue work                      # the built binary
```

`Bootstrap::run` reads the process's arguments: `serve` (the default), `queue work` or `schedule run`. With `queue.mode = "external"`, the serving process runs no worker.

## Act on the failed list

```bash
lat queue list
lat queue failed
lat queue retry 42
lat queue retry all
lat queue forget 42
lat queue flush --queue mail
```

```rust
use laterite_core::queue::JobFailed;
use laterite_core::{EventCx, EventError, Listener};

struct Alert;

#[laterite_core::strata::async_trait]
impl Listener<JobFailed> for Alert {
    async fn handle(&self, cx: &EventCx<'_>, failed: &JobFailed) -> Result<(), EventError> {
        acme::alerts::record(cx.db(), &failed.name, &failed.error).await?;
        Ok(())
    }
}

fn register(&self, registry: &mut Registry) {
    registry.listen::<JobFailed>(Alert);
}
```

A job reaching the failed list announces `queue.job_failed` with its `id`, `name`, `queue`, `attempts` and `error`.

## Grant the Jobs screen

```yaml
# a role's permissions
- backend.manage_jobs
```

Permission | Grants
--- | ---
`backend.manage_jobs` | **Settings → System → Jobs**: the waiting and running counts per queue, the failed list with each job's payload and error, Retry and Forget for one job or all.

## Test a job

```rust
let queue = Queue::inline(vec![JobReg::of::<SendDigest>()], services);
assert_eq!(queue.push(&SendDigest { user_id: 7 }).await?, Pushed::Ran);
```

```toml
# config/test.toml
[queue]
mode = "inline"
```
