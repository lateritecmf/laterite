//! Persistent row types for the auth schema.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// A backend user: an operator of the admin surface.
///
/// Names follow the structured `first_name` plus optional `last_name` pair, the
/// same shape the wider ecosystem standardizes on so integrations and any
/// future extensions can rely on the same fields. `password_hash` is skipped in
/// serialization so an identity can be handed to a template or API response
/// without leaking the stored credential.
#[derive(Debug, Clone, Serialize)]
pub struct BackendUser {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub first_name: String,
    pub last_name: Option<String>,
    #[serde(skip)]
    pub password_hash: String,
    pub is_superuser: bool,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
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
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub first_name: String,
    pub last_name: Option<String>,
    pub is_superuser: bool,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

/// The kind of event recorded in the access log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessEvent {
    LoginSuccess,
    LoginFailure,
    LockedOut,
    Logout,
}

impl AccessEvent {
    /// The stored string form; stable, since it is persisted.
    pub fn as_str(self) -> &'static str {
        match self {
            AccessEvent::LoginSuccess => "login_success",
            AccessEvent::LoginFailure => "login_failure",
            AccessEvent::LockedOut => "locked_out",
            AccessEvent::Logout => "logout",
        }
    }
}
