//! Shared `sea-query` identifiers for the auth schema, used by both the
//! migrations and the store so the two never drift on a table or column name.

use sea_query::Iden;

/// A long-lived "stay signed in" credential, one row per device.
#[derive(Iden)]
pub(crate) enum BackendRememberTokens {
    Table,
    /// Public lookup half of the cookie. Indexed, stored plain.
    Selector,
    /// SHA-256 of the secret half. Compared in constant time.
    VerifierHash,
    BackendUserId,
    CreatedAt,
    ExpiresAt,
}

#[derive(Iden)]
pub(crate) enum BackendUsers {
    Table,
    Id,
    Username,
    Email,
    FirstName,
    LastName,
    PasswordHash,
    IsSuperuser,
    IsActive,
    Timezone,
    Locale,
    Permissions,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
pub(crate) enum BackendRoles {
    Table,
    Id,
    Code,
    Name,
    Permissions,
    CreatedAt,
}

#[derive(Iden)]
pub(crate) enum BackendUserRoles {
    Table,
    BackendUserId,
    BackendRoleId,
}

#[derive(Iden)]
pub(crate) enum BackendSessions {
    Table,
    TokenHash,
    BackendUserId,
    CreatedAt,
    LastSeenAt,
    ExpiresAt,
    /// Opaque per-session blob (a serialised string the surface owns, e.g. the
    /// admin's CSRF token + flash). Auth never interprets it.
    Data,
    /// Where the session signed in from, subject to the trusted-proxy rule.
    IpAddress,
    /// The signing-in browser's user agent, capped by the surface.
    UserAgent,
}

#[derive(Iden)]
pub(crate) enum BackendAccessLog {
    Table,
    Id,
    BackendUserId,
    UsernameAttempted,
    Event,
    IpAddress,
    UserAgent,
    CreatedAt,
}

/// One operator's choice about how a screen is shown, keyed by screen.
#[derive(Iden)]
pub(crate) enum BackendUserPreferences {
    Table,
    Id,
    /// The operator the choice belongs to; the row goes with the account.
    UserId,
    /// What the choice is about, e.g. `list.columns./roles`.
    PreferenceKey,
    /// The stored value, shaped by whoever owns the key.
    Value,
}

#[derive(Iden)]
pub(crate) enum BackendAuditLog {
    Table,
    Id,
    /// The acting operator, nullable (a system action has none) and set null if
    /// the user is later removed.
    ActorUserId,
    /// The actor's username, snapshotted so the entry stays legible after the
    /// account is gone.
    ActorUsername,
    /// A dot-keyed action, e.g. `backend.role.update`.
    Action,
    /// What the action was on (e.g. `backend_role`) and its id, both optional.
    TargetType,
    TargetId,
    /// Optional JSON describing the change.
    Detail,
    CreatedAt,
}
