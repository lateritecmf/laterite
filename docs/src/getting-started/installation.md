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

`lat serve` runs the app from its directory and can override the bind address
without editing config: `lat serve --port 3000`, `lat serve --host 0.0.0.0`, or
`lat serve --listen 0.0.0.0:3000`. It passes the override through the standard
`APP__SERVER__LISTEN` environment variable.

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
│   ├── main.rs          # loads config, connects, migrates, serves the admin
│   └── migrations.rs    # this application's own migrations (empty to start)
└── storage/             # runtime data (the SQLite file, and later cache/logs)
```

`src/main.rs` is the whole application (abbreviated):

```rust
let config: AppConfig = laterite_core::config::load(Path::new("config"), "APP")?;
let db = laterite_core::db::connect(&config.database).await?;

// The framework's built-in migrations, then this application's own.
let mut sets = laterite_admin::builtin_migrations();
sets.extend(migrations::migrations());
laterite_core::migration::run(&db.pool, db.backend, &sets).await?;

let auth =
    laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default());
let router = laterite_admin::router(
    auth,
    db,
    Vec::new(), // application resources (list/form screens)
    Vec::new(), // application settings models
    Vec::new(), // application permissions
    admin_config,
);
axum::serve(listener, router).await?;
```

The three vectors are the application's own resources, [settings
models](../extend/settings.md), and [permissions](../extend/permissions.md); an
application with none yet passes them empty.

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

`lat doctor`, run from an application's directory, verifies it is ready to serve:
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
printed. No default password is ever shipped, and on a fresh install with no
accounts the admin also serves a one-time first-run setup screen instead of the
login form.
