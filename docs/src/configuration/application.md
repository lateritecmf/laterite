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
