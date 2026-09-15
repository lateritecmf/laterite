# Configuration

Laterite reads layered configuration, so one build runs across environments without code changes.
These files are deployment-level: per-install branding and per-operator preferences are edited in
the admin, not here (see [What is not configured here](#what-is-not-configured-here)).

## Layers

An application calls the loader with a config directory and an environment-variable prefix. Layers
apply in order, later overriding earlier:

1. `default.toml` (required): the base configuration.
2. `<APP_ENV>.toml` (optional): environment-specific overrides. `APP_ENV` selects the file and
   defaults to `development`. Create `staging.toml`, `production.toml`, `testing.toml`, and so on.
3. `local.toml` (optional): personal developer overrides, kept out of version control.
4. Environment variables `<PREFIX>__SECTION__KEY`: override any value (e.g. `LAT__DATABASE__URL`).
   The prefix is `LAT` unless the application declares its own as `app.env_prefix`; the `lat`
   command reads that declaration too, so the application and the tooling always agree.

So `APP_ENV=production` loads `default.toml` then `production.toml`. A `secure_cookie = true` in
`production.toml` turns the Secure cookie on only in that environment; environment variables win
over all files, which suits secrets and container deployments.

## Sections

```toml
[app]
name = "Acme Blog"               # display name, the baseline admin brand
env_prefix = "LAT"               # prefix of the overriding environment variables (LAT__SECTION__KEY)
# url = "https://acme.example"   # public base URL for absolute links; derived from listen when unset

[server]
listen = "127.0.0.1:8080"        # HTTP bind address

[database]
url = "postgres://localhost/acme_dev"
max_connections = 10             # optional
acquire_timeout_secs = 5         # optional

[backend]
secure_cookie = false            # set true behind HTTPS in production
timezone = "UTC"                 # default admin display timezone (IANA name); storage stays UTC
locale = "en"                    # default admin UI language; falls back to en if no catalog is loaded
path = "/admin"                  # URL path the admin panel mounts under; move or obscure it

[backend.paths]                  # move a module's admin screens, without forking it
"rainmill.location" = "/places"          # the whole module
"rainmill.location/nodes" = "/places"    # one screen; the more specific key wins

[auth]
session_idle_timeout_secs = 7200      # quiet time before a session ends, 2h default
session_absolute_timeout_secs = 43200 # ceiling counted from login, 12h default
remember_duration_secs = 1209600      # "stay signed in" credential, 14d default
max_failures = 5                 # failed logins before a username is locked out
failure_window_secs = 900        # window the failures are counted over
```

Every `[auth]` and `[backend]` key is optional and falls back to a built-in default when omitted.

A session runs on two clocks. `session_idle_timeout_secs` is measured from the
last request and moves forward as the operator works, so an active session does
not end mid-task. `session_absolute_timeout_secs` is measured from login and
never moves, capping how long a session can be kept alive. Whichever falls first
ends the session. `session_ttl_secs` was the earlier name for the ceiling and is
still read.

"Stay signed in" is a separate credential, not a longer session. Ticking the box
at login stores one row per device and sets a cookie that outlives the session;
when the session ends, that cookie mints a new one. Each use rotates the secret,
so a copy of the cookie works only until the real browser next uses it. If a
copy is used afterwards, the mismatch is taken as proof the cookie is in two
places and every credential for that account is dropped, signing both parties
out. `remember_duration_secs` sets how long an unused credential lasts, counted
from its last use.

## Where a module's screens mount

A module's admin screens namespace under its identity, so `rainmill.location`
contributing `/nodes` serves `/admin/rainmill/location/nodes` and two plugins
cannot collide by accident. A module may declare a shorter base of its own, and a
deployment has the last word through `backend.paths` above. Two claims on one path
abort the boot naming both, rather than one silently shadowing the other.

## How `lat` finds the application

Every `lat` command that acts on an application locates it by walking up from the current
directory to the nearest `config/default.toml`, so it works from any subdirectory. What the
application already states is read from there: `lat doctor` and `lat serve` load its configuration
under its declared prefix, and `lat admin` takes the database URL from `--database-url`, then
`DATABASE_URL`, then the application's `database.url`. A command asks for a flag only for what it
cannot find.

## What is not configured here

Deployment config is per-environment. Two related concerns live elsewhere, so they can change at
runtime without a redeploy:

- **Branding** (application name, colour-mode default, logo) is an operator-editable setting stored
  in the database and changed from the admin.
- **Preferences** are per-operator and set from the admin. An operator's own display timezone is
  one: `backend.timezone` is only the default until they choose their own from Preferences. See
  [Dates and Timezones](dates-and-timezones.md).
