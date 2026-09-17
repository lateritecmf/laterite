# Laterite

Laterite is a content management framework for Rust: a descriptor-driven admin
panel, authentication and permissions, namespaced migrations and typed
settings, on Axum and sqlx, for Postgres, MySQL or SQLite.

```bash
cargo install laterite-cli
lat new
```

Crate | Provides
--- | ---
`laterite-core` | Config, errors, the database layer, migrations, modules.
`laterite-auth` | Backend users, sessions, roles and permissions.
`laterite-admin` | The admin router, list and form screens, settings.
`laterite-media` | Content-addressed file storage. Optional.
`laterite-web` | Static-site generation and page metadata. Optional.
`laterite-cli` | The `lat` command.
`laterite-macros` | The `t!`, `tn!` and `tp!` translation macros.

Section | Covers
--- | ---
Getting Started | Installing, configuring and running an application.
Extending Laterite | Each capability: what to write and what can be set.
Reference | The icon set and the generated API documentation.
