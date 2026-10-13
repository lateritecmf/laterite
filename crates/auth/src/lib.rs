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

pub mod audit_listener;
pub mod error;
pub mod events;
pub mod migrations;
pub mod password;
pub mod permission;
pub mod service;
pub mod store;

/// The built-in role every administrative permission belongs to. Held by the
/// first operator, and by anyone who should administer the panel itself.
pub const ROLE_ADMIN: &str = "admin";

/// The built-in role for working with content rather than the panel. Empty on a
/// bare framework: a permission joins it by naming it.
pub const ROLE_EDITOR: &str = "editor";

mod models;
mod schema;

pub use audit_listener::AuditListener;
pub use error::AuthError;
pub use migrations::{migrations, MODULE_ID};
pub use models::{AccessEvent, BackendUser, BackendUserSummary};
pub use permission::PermissionSet;
pub use service::{
    ActiveSession, AuditEntry, AuthConfig, AuthService, AuthenticatedUser, IssuedSession,
    NewOperator, PasswordPolicy, Purged, RecalledSession, RememberCredential, RequestContext,
    ResolvedSession, MIN_PASSWORD_LENGTH,
};
pub use store::{AccessRecord, AuditRecord};
pub use store::{RevokeReason, SessionEnd};

/// The `laterite.auth` module: the framework's backend-user auth tables.
pub struct AuthModule;

#[laterite_core::strata::async_trait]
impl laterite_core::Module for AuthModule {
    fn id(&self) -> laterite_core::ModuleId {
        laterite_core::ModuleId::new(MODULE_ID)
    }
    /// Sweeps expired sessions and stay-signed-in credentials, so a boot
    /// starts from tables holding only what a request could still use.
    async fn boot(&self, cx: &laterite_core::BootCx<'_>) -> laterite_core::CoreResult<()> {
        let now = chrono::Utc::now();
        let sessions = store::delete_expired_sessions(cx.db, now)
            .await
            .map_err(|e| laterite_core::CoreError::Internal(e.to_string()))?;
        let tokens = store::delete_expired_remember_tokens(cx.db, now)
            .await
            .map_err(|e| laterite_core::CoreError::Internal(e.to_string()))?;
        if sessions + tokens > 0 {
            tracing::info!(
                sessions,
                remember_tokens = tokens,
                "purged expired credentials"
            );
        }
        Ok(())
    }
    fn migrations(&self) -> laterite_core::MigrationSet {
        migrations()
    }
    fn register(&self, registry: &mut laterite_core::Registry) {
        registry.add(laterite_core::ModelListenerReg::all(std::sync::Arc::new(
            audit_listener::AuditListener,
        )));
    }
}
