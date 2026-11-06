//! Persistent row types for the auth schema.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// A backend user: an operator of the admin surface.
///
/// Names follow the structured `first_name` plus optional `last_name` pair, the
/// same shape the wider ecosystem standardizes on so integrations and any
/// future extensions can rely on the same fields. `password_hash` is skipped in
/// serialization so an identity can be handed to a template or API response
/// without leaking the stored credential.
#[derive(Debug, Clone, Serialize)]
pub struct BackendUser {
    /// Id of the user's row in the backend users table.
    pub id: i64,
    /// Unique login name.
    pub username: String,
    /// Email address, used for invitations and password resets.
    pub email: String,
    /// Given name, the first part of [`BackendUser::full_name`].
    pub first_name: String,
    /// Family name, when given.
    pub last_name: Option<String>,
    /// Argon2id hash of the password in PHC string form; never serialized.
    #[serde(skip)]
    pub password_hash: String,
    /// Whether the user passes every permission check.
    pub is_superuser: bool,
    /// Whether the account may sign in; an inactive account is refused.
    pub is_active: bool,
    /// The operator's own display timezone as an IANA name (e.g. `Asia/Kolkata`).
    /// `None` means the operator inherits the deployment default. Storage is
    /// always UTC; this only affects how timestamps render for this operator.
    pub timezone: Option<String>,
    /// The operator's own UI language as a base tag (e.g. `kn`). `None` means the
    /// operator inherits the deployment default. This only affects the admin's own
    /// strings, not stored content.
    pub locale: Option<String>,
    /// When the account was created (UTC).
    pub created_at: DateTime<Utc>,
    /// When the account was last changed (UTC).
    pub updated_at: DateTime<Utc>,
}

impl BackendUser {
    /// The display name, derived rather than stored: `first_name` plus
    /// `last_name` when present.
    pub fn full_name(&self) -> String {
        match &self.last_name {
            Some(last) if !last.is_empty() => format!("{} {}", self.first_name, last),
            _ => self.first_name.clone(),
        }
    }
}

/// A lightweight backend-user projection for listings (no credential fields).
#[derive(Debug, Clone, Serialize)]
pub struct BackendUserSummary {
    /// Id of the user's row in the backend users table.
    pub id: i64,
    /// Unique login name.
    pub username: String,
    /// Email address.
    pub email: String,
    /// Given name.
    pub first_name: String,
    /// Family name, when given.
    pub last_name: Option<String>,
    /// Whether the user passes every permission check.
    pub is_superuser: bool,
    /// Whether the account may sign in.
    pub is_active: bool,
    /// When the account was created (UTC).
    pub created_at: DateTime<Utc>,
}

/// The kind of event recorded in the access log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccessEvent {
    /// A sign-in succeeded.
    LoginSuccess,
    /// A sign-in was refused: an unknown username, a wrong password, or a
    /// remember cookie that no longer matches.
    LoginFailure,
    /// A sign-in was turned away because too many attempts before it failed.
    LockedOut,
    /// The user ended their own session.
    Logout,
    /// The operator changed their own password, from the address recorded.
    PasswordChanged,
}

impl AccessEvent {
    /// The stored string form; stable, since it is persisted.
    pub fn as_str(self) -> &'static str {
        match self {
            AccessEvent::LoginSuccess => "login_success",
            AccessEvent::LoginFailure => "login_failure",
            AccessEvent::LockedOut => "locked_out",
            AccessEvent::Logout => "logout",
            AccessEvent::PasswordChanged => "password_changed",
        }
    }
}
