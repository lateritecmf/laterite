# Laterite

A reusable content management framework for Rust: descriptor-driven admin
screens, portable migrations, and role-based access, built on Axum and
`sea-query` so one codebase runs on Postgres, MySQL, or SQLite.

[![CI](https://github.com/lateritecmf/laterite/actions/workflows/ci.yml/badge.svg)](https://github.com/lateritecmf/laterite/actions/workflows/ci.yml)
[![SQLite](https://github.com/lateritecmf/laterite/actions/workflows/sqlite.yml/badge.svg)](https://github.com/lateritecmf/laterite/actions/workflows/sqlite.yml)
[![Postgres](https://github.com/lateritecmf/laterite/actions/workflows/postgres.yml/badge.svg)](https://github.com/lateritecmf/laterite/actions/workflows/postgres.yml)
[![MariaDB](https://github.com/lateritecmf/laterite/actions/workflows/mysql.yml/badge.svg)](https://github.com/lateritecmf/laterite/actions/workflows/mysql.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

## Quick start

Install the command-line tool and scaffold an application:

```
cargo install laterite-cli
lat new
```

`lat new` is a guided installer: it prompts for the application name, timezone,
and database, sets up the database, applies the migrations, and creates the first
administrator. Then run it:

```
cd acme
cargo run
```

Open <http://127.0.0.1:8080/admin> and sign in. Run `lat doctor` from the app
directory any time to check it is ready to serve.

## Highlights

- **Descriptor-driven admin.** Lists, forms, filters, and navigation are serde
  descriptor structs rendered by generic handlers, so admin screens are data
  rather than hand-written controllers.
- **Portable data layer.** Queries and migrations are built with `sea-query`
  over `sqlx::Any` and run unchanged on Postgres, MySQL, and SQLite. Backend
  quirks are hidden behind polyfills (booleans, key columns, casts, upserts), so
  you write natural Rust types. See the database portability guide.
- **Reversible migrations.** Each migration is a Rust unit with `up` and `down`,
  applied and rolled back by a runner that tracks history per module.
- **Role-based permissions.** Dotted permission strings carried on roles, with
  per-user overrides that take precedence, enforced by middleware.
- **Server-rendered admin.** Askama templates with HTMX, no JavaScript
  framework; Argon2id credentials and an append-only authentication log.

## Supported databases

Laterite compiles backend-agnostic; a deployment turns on exactly the driver it
runs against with a cargo feature, so no other database client is linked in:

| Feature | Database |
| --- | --- |
| `postgres` | PostgreSQL |
| `mysql` | MySQL / MariaDB |
| `sqlite` | SQLite |

The full suite runs against all three on every push (the badges above).

## Workspace

| Crate | What it is |
| --- | --- |
| `laterite-core` | kernel: config, errors, the `Db` handle, the migration engine, the query layer |
| `laterite-auth` | backend users, Argon2id credentials, sessions, and permissions |
| `laterite-admin` | the Axum admin router, descriptor screens, settings, and session middleware |
| `laterite-cli` | `lat`, the command-line tool |

Each crate builds, lints, and tests standalone.

## Developing the framework

To work on Laterite itself (rather than build an application with it), clone the
repository and run the workspace:

```
cargo build --workspace
cargo test --workspace --features sqlite
```

Tests run on an in-memory SQLite database by default. To run against Postgres or
MariaDB, point the harness at a server and enable that backend's feature:

```
LATERITE_TEST_DATABASE_URL=postgres://user@localhost/postgres \
  cargo test -p laterite-auth -p laterite-admin --features postgres
```

## Documentation

The guide is an mdBook under `docs/`; the API reference is rustdoc on each crate
(`cargo doc --open`).

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
