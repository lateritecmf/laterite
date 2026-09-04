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
4. Environment variables `LAT__SECTION__KEY`: override any value (e.g. `LAT__DATABASE__URL`). `LAT`
   is the default prefix; an application can set its own to namespace its variables.

So `APP_ENV=production` loads `default.toml` then `production.toml`. A `secure_cookie = true` in
`production.toml` turns the Secure cookie on only in that environment; environment variables win
over all files, which suits secrets and container deployments.

## Sections

```toml
[app]
name = "Acme Blog"               # display name, the baseline admin brand
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

[auth]
session_ttl_secs = 43200         # session lifetime, 12h default
max_failures = 5                 # failed logins before a username is locked out
failure_window_secs = 900        # window the failures are counted over
```

Every `[auth]` and `[backend]` key is optional and falls back to a built-in default when omitted.

## What is not configured here

Deployment config is per-environment. Two related concerns live elsewhere, so they can change at
runtime without a redeploy:

- **Branding** (application name, colour-mode default, logo) is an operator-editable setting stored
  in the database and changed from the admin.
- **Preferences** are per-operator and set from the admin. An operator's own display timezone is
  one: `backend.timezone` is only the default until they choose their own from Preferences. See
  [Dates and Timezones](dates-and-timezones.md).
