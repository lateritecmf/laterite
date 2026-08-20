# Laterite

Laterite is a content management framework for Rust. It gives an application a
descriptor-driven admin panel, authentication and permissions, a namespaced
migration system, and typed settings, all built on Axum, sqlx, and Postgres.

The framework is assembled from small crates, each owning one concern:

| Crate | Concern |
| --- | --- |
| `laterite-core` | Module registration and the migration runner |
| `laterite-auth` | Backend users, roles, sessions, and permissions |
| `laterite-admin` | Descriptor-driven list and form screens, and typed settings |
| `laterite-cli` | Administrative commands (create user, reset password) |

## How this guide is organized

- **Getting Started** walks through adding Laterite to a project and running
  the admin panel.
- **Extending Laterite** documents each capability as it is built: how to
  declare it, wire it into an application, and the guarantees it provides.
- **Reference** links to the generated API documentation for every crate.

This guide grows one feature at a time alongside the framework. If a capability
is not documented here yet, it is not part of a shipped release.
