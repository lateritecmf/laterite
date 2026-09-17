# Configuration

Configuration is layered: one build runs across environments without code
changes. Per-install branding and per-operator preferences are edited in the
admin, not here.

## Layers

Later layers override earlier ones.

Layer | Source
--- | ---
1 | `default.toml`. Required.
2 | `<APP_ENV>.toml`. Optional. `APP_ENV` defaults to `development`.
3 | `local.toml`. Optional; keep it out of version control.
4 | `<PREFIX>__SECTION__KEY` environment variables, e.g. `LAT__DATABASE__URL`. The prefix is `LAT` unless `app.env_prefix` names another.

## Sections

```toml
[app]
name = "Acme Blog"               # baseline admin brand
env_prefix = "LAT"               # prefix of the overriding environment variables
# url = "https://acme.example"   # public base URL; derived from listen when unset

[server]
listen = "127.0.0.1:8080"

[database]
url = "postgres://localhost/acme_dev"
max_connections = 10             # optional
acquire_timeout_secs = 5         # optional

[backend]
secure_cookie = false            # true behind HTTPS
timezone = "UTC"                 # default display timezone, IANA name
locale = "en"                    # default admin language
path = "/admin"                  # where the admin mounts
trusted_proxies = []             # CIDR ranges whose X-Forwarded-For is believed

[backend.paths]
"rainmill.location" = "/places"          # a whole module's screens
"rainmill.location/nodes" = "/places"    # one screen; the more specific key wins

[auth]
session_idle_timeout_secs = 7200      # 2h
session_absolute_timeout_secs = 43200 # 12h
remember_duration_secs = 1209600      # 14d
max_failures = 5
failure_window_secs = 900
```

Every `[auth]` and `[backend]` key is optional.

### `[backend]`

Key | Description
--- | ---
`secure_cookie` | Sets the Secure flag on cookies. `true` behind HTTPS.
`timezone` | Default display timezone until an operator picks their own. See [Dates and Timezones](dates-and-timezones.md).
`locale` | Default admin UI language. Falls back to `en`.
`path` | URL path the admin mounts under.
`trusted_proxies` | CIDR ranges. `X-Forwarded-For` is read only from a peer inside one; the client is the rightmost address not itself a trusted proxy. Empty records the peer address.
`paths` | Remounts a module's screens, or one screen, at another path. Two claims on one path abort the boot.

### `[auth]`

Key | Description
--- | ---
`session_idle_timeout_secs` | Quiet time before a session ends, from the last request. Default `7200`.
`session_absolute_timeout_secs` | Ceiling from login, never extended. Default `43200`. The earlier name `session_ttl_secs` is still read.
`remember_duration_secs` | How long an unused "stay signed in" credential lasts, from its last use. Default `1209600`.
`max_failures` | Failed logins before a username is locked out. Default `5`.
`failure_window_secs` | Window the failures are counted over. Default `900`.

## Where a module's screens mount

A module's screens namespace under its identity: `rainmill.location`
contributing `/nodes` serves `/admin/rainmill/location/nodes`. A module may
declare a shorter base; `backend.paths` has the last word.

## How `lat` finds the application

`lat` walks up from the current directory to the nearest `config/default.toml`
and reads the application's prefix from it. `lat admin` takes the database URL
from `--database-url`, then `DATABASE_URL`, then `database.url`.

## What is not configured here

Setting | Where
--- | ---
Branding: name, colour-mode default, logo | **Settings → Branding**
Preferences: an operator's own timezone | **Preferences**. See [Dates and Timezones](dates-and-timezones.md).
