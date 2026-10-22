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
storage = "storage"              # where files the admin keeps are stored
secret = "a-long-random-string"  # signs timed URLs; generated and stored if unset

[backend.order]
"backend.nav" = ["dashboard", "acme.shop", "media", "settings"]

[backend.menu]
"backend.access_log" = { in = "backend.nav", slot = "tools" }

[backend.paths]
"rainmill.location" = "/places"          # a whole module's screens
"rainmill.location/nodes" = "/places"    # one screen; the more specific key wins

[auth]
session_idle_timeout_secs = 7200      # 2h
session_absolute_timeout_secs = 43200 # 12h
remember_duration_secs = 1209600      # 14d
max_failures = 5
max_failures_per_address = 20
failure_window_secs = 900

[auth.password_policy]
min_length = 8
```

Every `[auth]` and `[backend]` key is optional.

### `[backend]`

Key | Description
--- | ---
`secure_cookie` | Sets the Secure flag on cookies and sends `Strict-Transport-Security` (one year, subdomains included). `true` behind HTTPS.
`timezone` | Default display timezone until an operator picks their own. See [Dates and Timezones](dates-and-timezones.md).
`locale` | Default admin UI language. Falls back to `en`.
`path` | URL path the admin mounts under.
`trusted_proxies` | CIDR ranges. `X-Forwarded-For` is read only from a peer inside one; the client is the rightmost address not itself a trusted proxy. Empty records the peer address.
`paths` | Remounts a module's screens, or one screen, at another path. Two claims on one path abort the boot.
`skin` | The admin skin by code: `standard` (default), `compact`, or one a module registered. See [Skins](../extend/skins.md).
`storage` | The directory files the admin keeps are stored under, relative to the working directory unless absolute. Uploaded import files go to `private` beneath it.
`order` | Arrangement overrides by hierarchy: `"backend.nav"`, `"backend.nav.<section>"`, `"backend.settings"` or a plugin's own, each a list of keys in the wanted order. See [Navigation](../extend/navigation.md).
`menu` | Entries moved out of their home, keyed by entry: `{ in, slot, order, category }`. See [Navigation](../extend/navigation.md).
`secret` | The key timed URLs are signed with (a private file's). Unset, one is generated at the first boot and kept in the settings table. Set it when several instances share one database.

### `[auth]`

Key | Description
--- | ---
`session_idle_timeout_secs` | Quiet time before a session ends, from the last request. Default `7200`.
`session_absolute_timeout_secs` | Ceiling from login, never extended. Default `43200`. The earlier name `session_ttl_secs` is still read.
`remember_duration_secs` | How long an unused "stay signed in" credential lasts, from its last use. Default `1209600`.
`max_failures` | Failed sign-ins from one address for one username before that address is locked out of it. Default `5`.
`max_failures_per_address` | Failed sign-ins from one address, any username, before the address is locked out. Default `20`.
`failure_window_secs` | Window the failures are counted over. Default `900`.
`password_policy.min_length` | The shortest password accepted, wherever one is set. Default `8`.

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

## Branding

Settings › Branding, saved by an operator with `backend.manage_branding`.

Field | Takes | Effect
--- | --- | ---
Application name | text | the brand in the top bar, drawer, sign-in page and titles; blank keeps `app.name`
Logo | a library image | shown in place of the mark, scaled to 28px high (40px on the sign-in page)
Browser icon | a library image | the tab icon
Accent colour | `#rrggbb` | `--accent`, its pressed shades and tints, and link colours
Colour mode | `auto`, `light`, `dark` | what an operator sees before choosing their own
Sign-in prompt | text | the line above the sign-in form
Sign-in tagline | text | the line on the panel beside the form
Sign-in background | a library image | behind that panel
Show the panel | on, off | off leaves the form alone, centred
Custom stylesheet | CSS | added to every admin page; `@import`, scripts and closing tags are removed

Library images must sit on a public disk: a private disk's signed URL would expire in the page.
