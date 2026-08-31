//! Laterite auth: backend user authentication and authorization.
//!
//! Provides the operator-facing security primitives the admin surface is
//! built on: Argon2id password hashing, opaque server-side sessions, a
//! role-based permission model over dotted permission strings, brute-force
//! throttling, and an append-only access log. "Backend users" are the
//! operators of the admin, kept distinct from any application's end users.
//!
//! This crate is HTTP-agnostic on purpose. It exposes an [`AuthService`] with
//! plain async methods (`authenticate`, `verify_session`, `logout`) plus an
//! [`AuthenticatedUser`] identity; the admin crate wraps these in Axum
//! extractors, cookie handling, and the rendered login screen.

pub mod error;
pub mod migrations;
pub mod password;
pub mod permission;
pub mod service;
pub mod store;

mod models;
mod schema;

pub use error::AuthError;
pub use migrations::{migrations, MODULE_ID};
pub use models::{AccessEvent, BackendUser, BackendUserSummary};
pub use permission::PermissionSet;
pub use service::{
    AuthConfig, AuthService, AuthenticatedUser, IssuedSession, NewOperator, RequestContext,
};

/// The `laterite.auth` module: the framework's backend-user auth tables.
pub struct AuthModule;

impl laterite_core::Module for AuthModule {
    fn id(&self) -> laterite_core::ModuleId {
        laterite_core::ModuleId::new(MODULE_ID)
    }
    fn migrations(&self) -> laterite_core::MigrationSet {
        migrations()
    }
}
