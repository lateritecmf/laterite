# Installation

Laterite runs on stable Rust and Postgres. An application depends on the crates
it needs, runs their migrations, and mounts the admin router.

## Prerequisites

- Rust (stable), installed with [rustup](https://rustup.rs).
- A Postgres database and a `DATABASE_URL` connection string.

## Add the crates

Add the crates your application uses to its `Cargo.toml`. A typical admin
application depends on the core, auth, admin, and settings crates:

```toml
[dependencies]
laterite-core = "0.1"
laterite-auth = "0.1"
laterite-admin = "0.1"
laterite-settings = "0.1"
```

## Run migrations

Each crate owns its migrations and exposes them as a `ModuleMigrations` set. The
runner records applied migrations per module so each set advances independently.
`laterite_admin::builtin_migrations()` bundles the sets for every module the
admin mounts (the auth schema and the settings store), so an application runs one
call instead of naming each framework module by hand:

```rust
laterite_core::migrate::run(&pool, &laterite_admin::builtin_migrations()).await?;
```

An application with its own modules appends their sets before running:

```rust
let mut migrations = laterite_admin::builtin_migrations();
migrations.extend([pages::migrations()]);
laterite_core::migrate::run(&pool, &migrations).await?;
```

## Mount the admin panel

`laterite_admin::router` returns an Axum router with the built-in screens
(users and roles) plus any resources the application registers. Serve it like
any other Axum service:

```rust
let app = laterite_admin::router(
    auth,
    pool,
    Vec::new(), // resources
    Vec::new(), // settings items
    Vec::new(), // permissions
    laterite_admin::AdminConfig::default(),
);
let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
axum::serve(listener, app).await?;
```

The three vectors are the application's own [resources](../extend/settings.md),
settings items, and [permissions](../extend/permissions.md); an application with
none yet passes them empty.

## Create the first administrator

With the server running, open `/admin`. On a fresh install with no accounts, the
admin serves a **first-run setup** screen instead of login: fill in the first
administrator (username, name, email, password) and a display timezone, and you
are signed straight in. The setup screen closes itself once an account exists, so
it is only reachable while the install is empty. No default password is ever
shipped.

For a scripted or headless install, create the first administrator from the
command line instead. The `laterite-cli` crate installs a `lat` binary:

```bash
cargo install laterite-cli
lat admin create admin --email admin@acme.test --first-name Admin
```

The command prompts for a password, or pass `--generate` to have a strong one
created and printed.
