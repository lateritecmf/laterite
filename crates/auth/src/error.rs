//! Auth error taxonomy.

use laterite_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    /// Username unknown, or password did not match. Deliberately does not
    /// distinguish the two, so the response cannot be used to enumerate users.
    #[error("invalid credentials")]
    InvalidCredentials,

    /// Too many recent failures for this identity; login is temporarily locked.
    #[error("too many attempts")]
    TooManyAttempts,

    /// No such session, or it has expired.
    #[error("session invalid")]
    SessionInvalid,

    /// Credentials were correct but the account is disabled.
    #[error("account is inactive")]
    InactiveAccount,

    /// The authenticated backend user lacks the required permission.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// A deactivation that would leave the panel without a way in, or that an
    /// operator aimed at their own account. Carries the reason, which is shown
    /// to the operator rather than logged and swallowed.
    #[error("{0}")]
    Refused(String),

    /// Password hashing or verification failed at the cryptographic layer.
    #[error("password hashing failure")]
    PasswordHash(String),

    #[error("database error")]
    Store(#[from] sqlx::Error),

    /// A stored value could not be parsed back into its Rust type (corrupt data
    /// or a schema mismatch).
    #[error("corrupt stored data: {0}")]
    Data(String),
}

impl From<AuthError> for CoreError {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::InvalidCredentials
            | AuthError::TooManyAttempts
            | AuthError::SessionInvalid => CoreError::Unauthorized,
            AuthError::InactiveAccount => CoreError::Forbidden("account is inactive".to_string()),
            AuthError::PermissionDenied(perm) => CoreError::Forbidden(perm),
            // The operator asked for something the rules refuse; the reason is
            // theirs to read, so it travels rather than becoming a bare 403.
            AuthError::Refused(reason) => CoreError::Forbidden(reason),
            AuthError::PasswordHash(msg) => CoreError::Internal(msg),
            AuthError::Store(e) => CoreError::Database(e),
            AuthError::Data(msg) => CoreError::Internal(msg),
        }
    }
}
