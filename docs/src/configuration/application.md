# Application and Server

Configuration is layered: one build runs across environments without code
changes. This page covers the layers, `[app]` and `[server]`.

## Layer the files

Later layers override earlier ones.

Layer | Source
--- | ---
1 | `default.toml`. Required.
2 | `<APP_ENV>.toml`. Optional. `APP_ENV` defaults to `development`.
3 | `local.toml`. Optional; keep it out of version control.
4 | `<PREFIX>__SECTION__KEY` environment variables, e.g. `LAT__DATABASE__URL`. The prefix is `LAT` unless `app.env_prefix` names another.

Section | Page
--- | ---
`[database]` | [Database](database.md)
`[backend]` | [Admin](admin.md)
`[auth]` | [Authentication](authentication.md)

## Set `[app]` and `[server]`

```toml
[app]
name = "Acme Blog"               # baseline admin brand
env_prefix = "LAT"               # prefix of the overriding environment variables
locale = "en"                    # default admin language
debug = false                    # error pages show the cause
# url = "https://acme.example"   # public base URL; derived from listen when unset

[server]
listen = "127.0.0.1:8080"
```

Key | Description
--- | ---
`app.name` | The application name, the baseline admin brand. [Branding](../admin/branding.md) overrides it. Default `Laterite`.
`app.url` | The public base URL absolute links build on, scheme and host with any port. Derived from `server.listen` when unset.
`app.locale` | Default admin UI language, a BCP 47 tag. Default `en`. See [Localization](../services/localization.md).
`app.debug` | Error pages include the cause. Off by default. See [Errors and Logging](errors-and-logging.md).
`app.env_prefix` | The prefix of the overriding environment variables. Default `LAT`.
`app.key` | The application key, `id:<base64 of 32 bytes>`; `lat key generate` writes one. See [Encryption and Signing](../services/crypt.md).
`app.previous_keys` | Keys retired by a rotation, each `id:<base64>`, read but never written with.
`server.listen` | The `host:port` to bind. Default `127.0.0.1:8080`. `lat serve --host`, `--port` and `--listen` override it.

## Set `[cache]`

```toml
[cache]
store = "auto"            # auto | null | memory | redis
memory_mib = 64
poll_seconds = 2
# url = "redis://127.0.0.1:6379/0"
```

Key | Description
--- | ---
`cache.store` | `auto` is `null` while `app.debug` is on and `memory` otherwise. See [Cache](../services/cache.md).
`cache.memory_mib` | The `memory` store's capacity. Default `64`.
`cache.poll_seconds` | How often a process reads the other processes' invalidations; `0` for a single process. Default `2`.
`cache.url` | The Redis URL for the `redis` store.
`cache.prefix` | The prefix of every Redis key. Default `laterite`.

## Set `[queue]`

```toml
[queue]
store = "auto"            # auto | database | redis
mode = "worker"           # worker | external | inline
workers = 2
queues = ["default"]
retry_after_seconds = 90
```

Key | Description
--- | ---
`queue.store` | Where jobs wait; `auto` is `database`. See [Queue](../services/queue.md).
`queue.mode` | `worker` in the application process, `external` for `queue work` alone, `inline` to run a job as it is pushed.
`queue.workers` | Workers per process in `worker` mode. Default `2`.
`queue.queues` | The queues taken from, in priority order. Default `["default"]`.
`queue.retry_after_seconds` | When a claimed job counts as abandoned. Default `90`.
`queue.poll_seconds` | An idle worker's wait. Default `1`.

## Set `[schedule]`

```toml
[schedule]
mode = "ticker"           # ticker | external | off
```

Key | Description
--- | ---
`schedule.mode` | `ticker` in the application process, `external` for `schedule run` alone, `off` for none. See [Scheduler](../services/schedule.md).

## Set `[mail]`

```toml
[mail]
driver = "smtp"            # smtp | log | stub
from = "Acme <no-reply@acme.example>"

[mail.smtp]
host = "smtp.acme.example"
port = 587
encryption = "starttls"
username = ""
password = ""
```

Key | Description
--- | ---
`mail.driver` | `smtp`, `log` or `stub`; the Mail settings screen lays over it. See [Mail](../services/mail.md).
`mail.from` | The sender, `Name <address>`.
`mail.to` | A development override for every recipient.
`mail.smtp.*` | `host`, `port`, `encryption` (`starttls`, `tls`, `none`), `username`, `password`, `timeout_seconds`.

## Set `[theme]`

```toml
[theme]
folder = "themes/acme"
copy = ["static:static"]
```

Key | Default | Meaning
--- | --- | ---
`folder` | none | A static website's theme folder, relative to the project. Set, `lat serve` previews it and `lat theme build` renders it. See [Static Website](../getting-started/static-website.md).
`copy` | `[]` | Directories copied into the build output, `<src>:<dest>`, relative to the project; one absent on this machine is skipped with a note.

## Mount a module's screens

A module's screens namespace under its identity: `acme.places`
contributing `/nodes` serves `/admin/acme/places/nodes`. A module may
declare a shorter base; [`backend.paths`](admin.md) has the last word.

## Locate the application from `lat`

`lat` walks up from the current directory to the nearest `config/default.toml`
and reads the application's prefix from it. `lat admin` takes the database URL
from `--database-url`, then `DATABASE_URL`, then `database.url`.

## Find what is not configured here

Setting | Where
--- | ---
Branding: name, colour-mode default, logo | [Branding](../admin/branding.md)
Preferences: a user's own timezone | **Preferences**. See [Dates and Timezones](dates-and-timezones.md).
