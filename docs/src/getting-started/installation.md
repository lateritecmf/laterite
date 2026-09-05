# Installation

The fastest way to a running Laterite application is the `lat` command-line
tool, which scaffolds a project, sets up the database, and creates the first
administrator in one guided step.

## Prerequisites

- Rust (stable), installed with [rustup](https://rustup.rs).
- One of PostgreSQL, MySQL/MariaDB, or SQLite. SQLite needs nothing extra; it is
  just a file.

## Install the CLI

```bash
cargo install laterite-cli
```

This installs the `lat` binary. It bundles every database driver, so one binary
works with any supported database.

## Create an application

Run `lat new` and answer the prompts:

```bash
lat new
```

It asks for:

- the **application name** (any text, such as `Acme Blog`). The installer
  derives a project slug from it (`acme-blog`) for the crate, directory, and
  database, so you never slugify by hand. The name itself is saved in config and
  shown as the admin brand,
- a **display timezone** (type to search the IANA list),
- a **listen address** (`host:port`, default `127.0.0.1:8080`),
- a **database** (PostgreSQL, MySQL/MariaDB, or SQLite) and its connection
  details, offering to create the database if it does not exist,
- the **first administrator** (username, email, password).

It scaffolds the project, applies the framework migrations, and creates the
administrator. When it finishes:

```bash
cd acme
cargo run          # or: lat serve
```

Open <http://127.0.0.1:8080/admin> and sign in.

`lat serve` runs the app from anywhere inside it and can override the bind address
without editing config: `lat serve --port 3000`, `lat serve --host 0.0.0.0`, or
`lat serve --listen 0.0.0.0:3000`. It passes the override through the app's
`<PREFIX>__SERVER__LISTEN` environment variable (`LAT` unless the app declares
`app.env_prefix`, see [Configuration](configuration.md)).

## What it generates

The project stands alone (its own Cargo workspace) and follows a small
convention:

```text
acme/
├── Cargo.toml
├── config/
│   ├── default.toml     # committed defaults (app name, listen address, timezone)
│   └── local.toml       # git-ignored; holds the database URL
├── src/
│   ├── main.rs          # hands off to Bootstrap: config, connect, migrate, serve
│   └── migrations/      # this application's own migrations (empty to start)
└── storage/             # runtime data (the SQLite file, and later cache/logs)
```

`src/main.rs` is the whole application. It hands off to `Bootstrap`, which loads
config, connects the database, runs the built-in and application migrations, and
serves the admin:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    laterite_admin::Bootstrap::new("config")
        .app_migrations(vec![migrations::migrations()])
        // .resources(...).settings(...).permissions(...)
        // .extend(|router, ctx| router.merge(my_api(ctx.db())))
        .serve()
        .await
}
```

Keeping the boot behind `Bootstrap` means framework internals can change without
touching `main.rs`. The builder is where an application registers its own resources (list/form
screens), [settings models](../extend/settings.md), and
[permissions](../extend/permissions.md), and `extend` merges its own routes (a
public API, web pages) onto the admin router; an application with none yet
registers nothing.

## The application name and brand

The name you entered is saved in `config/default.toml`:

```toml
[app]
name = "Acme Blog"
```

It is shown as the brand across the admin (the top nav, the sign-in screen, page
titles). It is the baseline: an administrator can override it under **Settings →
Branding**, and that setting takes precedence. When both are blank the brand
falls back to `Laterite`. This mirrors the config-then-setting layering: the
config value is the default, the setting overrides it.

## Check the setup

`lat doctor`, run from anywhere inside an application, verifies it is ready to serve:
the configuration loads, the timezone is valid, `storage/` is writable, the
database is reachable, and the framework's tables are present. It exits non-zero
if anything fails, so it can gate a deploy:

```bash
cd acme
lat doctor
```

## Managing administrators later

The first administrator is created during `lat new`. To add or recover one later
(a scripted install, or a forgotten password), use the CLI directly against the
application's database:

```bash
lat admin create editor --email editor@acme.test --first-name Editor
lat admin reset-password editor
```

Both prompt for a password, or pass `--generate` to have a strong one created and
printed. Run inside the application, they read its database URL from the
configuration; elsewhere pass `--database-url` or set `DATABASE_URL`. No default
password is ever shipped, and on a fresh install with no accounts the admin also
serves a one-time first-run setup screen instead of the login form.
