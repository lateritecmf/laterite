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
