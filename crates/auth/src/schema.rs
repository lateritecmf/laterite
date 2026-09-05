//! Shared `sea-query` identifiers for the auth schema, used by both the
//! migrations and the store so the two never drift on a table or column name.

use sea_query::Iden;

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
