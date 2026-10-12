//! Laterite CMF: the one crate an application depends on.
//!
//! The framework is a family of crates with one version. This crate carries no
//! code of its own: it re-exports each crate under a short name and forwards the
//! database driver features, so an application names one dependency, pins one
//! version, and reaches everything through one path.
//!
//! ```toml
//! [dependencies]
//! lateritecmf = { version = "0.9", features = ["postgres"] }
//! ```
//!
//! ```rust,ignore
//! use lateritecmf::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     Bootstrap::new("config").module(AppModule).serve().await
//! }
//! ```
//!
//! A plugin depends on the crates it uses directly (`laterite-core`,
//! `laterite-admin`), which keeps its dependency light; the umbrella is for the
//! application that assembles them.

/// The platform library: config, the database, migrations, the record layer,
/// events, validation, localization, modules and the registry.
pub use laterite_core as core;

/// Operators, roles, permissions, sessions and the audit table.
pub use laterite_auth as auth;

/// The admin panel: the bootstrap, resources, lists, forms, fields, import,
/// export, settings and plugins.
pub use laterite_admin as admin;

/// File storage: drivers, ingest and media records.
pub use laterite_media as media;

/// The public web layer: static site generation, page metadata and sitemaps.
pub use laterite_web as web;

/// The names an application uses on nearly every page.
pub mod prelude {
    pub use laterite_admin::form::{FormConfig, FormField};
    pub use laterite_admin::list::{ListColumn, ListConfig};
    pub use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
    pub use laterite_admin::{
        resource, AdminConfig, AdminRegistry, Bootstrap, BootstrapCtx, ImportConfig, Permission,
        Resource,
    };
    pub use laterite_core::{migration_set, t, Db, MigrationSet, Module, ModuleId, Registry, Text};
}
