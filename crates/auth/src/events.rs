//! The facts the auth service announces on the event bus.
//!
//! Each is emitted once it is true: after the row that records it is written.
//! Listen from a module's `register`:
//!
//! ```ignore
//! registry.listen::<laterite_auth::events::SignedIn>(WelcomeBack);
//! ```

use laterite_core::Event;
use serde::{Deserialize, Serialize};

use crate::service::RequestContext;

/// `auth.signed_in`: an operator signed in, by password or by a stay-signed-in
/// credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SignedIn {
    /// Id of the backend user who signed in.
    pub user_id: i64,
    /// Username of the account that signed in.
    pub username: String,
    /// The address the request came from, when known.
    pub ip_address: Option<String>,
    /// The `User-Agent` header of the request, when sent.
    pub user_agent: Option<String>,
}

impl SignedIn {
    /// Builds the event for `user_id`, taking the address and agent from `ctx`.
    pub fn new(user_id: i64, username: impl Into<String>, ctx: &RequestContext) -> Self {
        Self {
            user_id,
            username: username.into(),
            ip_address: ctx.ip_address.clone(),
            user_agent: ctx.user_agent.clone(),
        }
    }
}

impl Event for SignedIn {
    const NAME: &'static str = "auth.signed_in";
}

/// `auth.sign_in_failed`: a credential was refused. Counts toward the lockout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SignInFailed {
    /// The account the attempt named, `None` when no such account exists.
    pub user_id: Option<i64>,
    /// The username as it was typed. Empty for a refused stay-signed-in
    /// credential, which names no username.
    pub username: String,
    /// The address the request came from, when known.
    pub ip_address: Option<String>,
    /// The `User-Agent` header of the request, when sent.
    pub user_agent: Option<String>,
}

impl SignInFailed {
    /// Builds the event for the attempt, taking the address and agent from `ctx`.
    pub fn new(user_id: Option<i64>, username: impl Into<String>, ctx: &RequestContext) -> Self {
        Self {
            user_id,
            username: username.into(),
            ip_address: ctx.ip_address.clone(),
            user_agent: ctx.user_agent.clone(),
        }
    }
}

impl Event for SignInFailed {
    const NAME: &'static str = "auth.sign_in_failed";
}

/// `auth.locked_out`: an attempt was turned away because too many before it
/// failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LockedOut {
    /// The account the attempt named, `None` when no such account exists.
    pub user_id: Option<i64>,
    /// The username as it was typed. Empty for a stay-signed-in credential.
    pub username: String,
    /// The address the request came from, when known.
    pub ip_address: Option<String>,
    /// The `User-Agent` header of the request, when sent.
    pub user_agent: Option<String>,
}

impl LockedOut {
    /// Builds the event for the attempt, taking the address and agent from `ctx`.
    pub fn new(user_id: Option<i64>, username: impl Into<String>, ctx: &RequestContext) -> Self {
        Self {
            user_id,
            username: username.into(),
            ip_address: ctx.ip_address.clone(),
            user_agent: ctx.user_agent.clone(),
        }
    }
}

impl Event for LockedOut {
    const NAME: &'static str = "auth.locked_out";
}

/// `auth.signed_out`: an operator ended their own session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SignedOut {
    /// Id of the backend user who signed out.
    pub user_id: i64,
}

impl SignedOut {
    /// Builds the event for `user_id`.
    pub fn new(user_id: i64) -> Self {
        Self { user_id }
    }
}

impl Event for SignedOut {
    const NAME: &'static str = "auth.signed_out";
}

/// `auth.password_changed`: an account has a new password, and every other
/// session it held has ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PasswordChanged {
    /// Id of the backend user whose password changed.
    pub user_id: i64,
    /// The operator who changed it: the account itself, or an administrator.
    /// `None` when a process did, such as the command line.
    pub changed_by: Option<i64>,
}

impl PasswordChanged {
    /// Builds the event for `user_id`, changed by `changed_by`.
    pub fn new(user_id: i64, changed_by: Option<i64>) -> Self {
        Self {
            user_id,
            changed_by,
        }
    }

    /// Whether the account changed its own password.
    pub fn by_owner(&self) -> bool {
        self.changed_by == Some(self.user_id)
    }
}

impl Event for PasswordChanged {
    const NAME: &'static str = "auth.password_changed";
}
