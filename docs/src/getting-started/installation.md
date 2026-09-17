# Installation

`lat new` scaffolds a project, sets up the database and creates the first
administrator.

## Install the CLI

```bash
cargo install laterite-cli
```

Requires stable Rust and one of PostgreSQL, MySQL/MariaDB or SQLite.

## Create an application

```bash
lat new
cd acme
cargo run          # or: lat serve
```

Open <http://127.0.0.1:8080/admin> and sign in.

`lat new` asks for:

Prompt | Value
--- | ---
**Application name** | Any text, such as `Acme Blog`. Its slug, `acme-blog`, names the crate, directory and database.
**Display timezone** | An IANA name; type to search.
**Listen address** | `host:port`. Default `127.0.0.1:8080`.
**Database** | PostgreSQL, MySQL/MariaDB or SQLite, and its connection details. Offers to create the database.
**First administrator** | Username, email, password.

## Serve

```bash
lat serve
lat serve --port 3000
lat serve --host 0.0.0.0
lat serve --listen 0.0.0.0:3000
```

Runs from anywhere inside the application. An override sets
`<PREFIX>__SERVER__LISTEN`; see [Configuration](configuration.md).

## What it generates

```text
acme/
├── Cargo.toml
├── README.md
├── .gitignore
├── config/
│   ├── default.toml     # committed defaults
│   └── local.toml       # git-ignored; the database URL
├── src/
│   ├── main.rs
│   └── migrations/      # the application's module and its migrations
└── storage/             # runtime data
```

`src/main.rs`:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    laterite_admin::Bootstrap::new("config")
        .module(migrations::AppModule)
        // .extend(|router, ctx| router.merge(my_api(ctx.db())))
        .serve()
        .await
}
```

`src/migrations/mod.rs` defines the application's module:

```rust
impl laterite_core::Module for AppModule {
    fn id(&self) -> laterite_core::ModuleId {
        laterite_core::ModuleId::new(MODULE_ID)
    }
    fn migrations(&self) -> laterite_core::MigrationSet {
        migrations()
    }
    fn register(&self, registry: &mut laterite_core::Registry) {
        use laterite_admin::AdminRegistry;
        // registry.add_resource(..);
        // registry.add_permission(..);
        // registry.add_settings(..);
    }
}
```

`register` contributes resources, [settings models](../extend/settings.md) and
[permissions](../extend/permissions.md). `Bootstrap::extend` merges the
application's own routes onto the admin router.

## Set the brand

```toml
[app]
name = "Acme Blog"
```

Shown across the admin. **Settings → Branding** overrides it; with both blank
the brand is `Laterite`.

## Check the setup

```bash
lat doctor
```

Verifies the configuration, the timezone, write access to `storage/`, the
database connection and the framework tables. Exits non-zero on any failure.

## Manage administrators

```bash
lat admin create editor --email editor@acme.test --first-name Editor
lat admin reset-password editor
```

Both prompt for a password; `--generate` prints a strong one. Outside the
application, pass `--database-url` or set `DATABASE_URL`. A fresh install with
no accounts serves a first-run setup screen at `/admin`.
