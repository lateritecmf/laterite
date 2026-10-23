# Scheduler

An entry names a cron expression and what to do; the application evaluates every entry each minute, in the entry's timezone, on one process of the deployment.

```rust
use laterite_core::schedule::{Overlap, ScheduleEntry, ScheduleRegistry};

fn register(&self, registry: &mut Registry) {
    registry.add_schedule(
        ScheduleEntry::new("acme.blog::digest", "0 7 * * *")
            .timezone("Europe/Berlin")
            .description("Send the daily digest.")
            .job(SendDigest { user_id: 0 }),
    );
    registry.add_schedule(
        ScheduleEntry::every_minutes("acme.blog::sweep", 5)
            .run(|cx| async move { sweep(cx.db()).await }),
    );
}
```

Constructor | Expression
--- | ---
`ScheduleEntry::new(name, expression)` | Five fields (`0 7 * * *`), six with seconds, or `@hourly`, `@daily`, `@weekly`, `@monthly`, `@yearly`.
`ScheduleEntry::every_minutes(name, n)` | `*/n * * * *`
`ScheduleEntry::hourly(name)` | `0 * * * *`
`ScheduleEntry::daily_at(name, "07:30")` | `30 7 * * *`

Method | Default | Meaning
--- | --- | ---
`.run(\|cx\| async { .. })` | | Runs the closure when due; `cx` gives `db()`, `events()`, `cache()`, `parameters()`, `queue()`.
`.job(job)` | | Pushes the job when due.
`.timezone("Asia/Colombo")` | `UTC` | The timezone the expression is read in.
`.description("..")` | | A line for the Scheduled tasks screen.
`.overlap(Overlap::Allow)` | `Overlap::Skip` | Whether a due entry runs while its last run is still going.
`.one_server(false)` | `true` | Whether one process of the deployment runs it per minute, or every process does.

An entry with a bad expression or timezone, without `.run` or `.job`, or with a name already taken is refused when the application boots.

## Configure the ticker

```toml
# config/default.toml
[schedule]
mode = "ticker"           # ticker | external | off
```

Key | Description
--- | ---
`schedule.mode` | `ticker` evaluates the entries in the application process; `external` leaves it to `schedule run`; `off` evaluates nothing.

## Run the ticker apart

```bash
lat schedule run                     # cargo run -- schedule run
lat schedule run --once              # this minute's entries, then exit
acme schedule run                    # the built binary
```

```cron
* * * * * cd /srv/acme && ./acme schedule run --once
```

`Bootstrap::run` reads the process's arguments: `serve` (the default), `queue work` or `schedule run`.

## Read the last runs

```bash
lat schedule list
```

```rust
for entry in ctx.schedule().list().await? {
    println!("{} next {:?} last {:?}", entry.name, entry.next_run_at, entry.last_status);
}
ctx.schedule().run_now("acme.blog::digest").await?;
```

Field | Holds
--- | ---
`name`, `expression`, `timezone`, `description` | The entry as declared.
`next_run_at` | The next due minute, UTC.
`last_started_at`, `last_finished_at` | The last run's bounds, UTC.
`last_status` | `never`, `running`, `ok`, `failed` or `skipped`.
`last_error` | The last run's error, when it failed.

`run_now(name)` runs an entry outside its expression, under the same locks; `false` when it was skipped.

## Grant the Scheduled tasks screen

```yaml
# a role's permissions
- backend.manage_schedule
```

Permission | Grants
--- | ---
`backend.manage_schedule` | **Settings → System → Scheduled Tasks**: every entry with its expression, timezone, next and last run, status and error, and Run now.

## Entries the framework registers

Entry | Expression | Does
--- | --- | ---
`laterite.auth::purge` | `0 * * * *` | Removes expired sessions and stay-signed-in credentials.
