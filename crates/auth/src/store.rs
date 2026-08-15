//! Data access for the auth schema.
//!
//! Free functions over a `PgPool`. Query text is checked against the database
//! at compile time by the sqlx macros; the service layer composes these into
//! the authentication and authorization flows.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AuthError;
use crate::models::{AccessEvent, BackendUser, BackendUserSummary};

/// Looks up a user by username without filtering on active state, so the
/// caller can distinguish a disabled account from a wrong password only after
/// the password has been verified.
pub async fn find_user_by_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<BackendUser>, AuthError> {
    let user = sqlx::query_as!(
        BackendUser,
        r#"select id, username, email, first_name, last_name, password_hash,
                  is_superuser, is_active, timezone, created_at, updated_at
           from backend_users
           where username = $1"#,
        username
    )
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

/// Looks up an active user by id, used when resolving a session to an identity.
pub async fn find_active_user_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<BackendUser>, AuthError> {
    let user = sqlx::query_as!(
        BackendUser,
        r#"select id, username, email, first_name, last_name, password_hash,
                  is_superuser, is_active, timezone, created_at, updated_at
           from backend_users
           where id = $1 and is_active = true"#,
        id
    )
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

/// Returns the permission arrays of every role assigned to a user, for the
/// service to flatten into a permission set.
pub async fn load_role_permissions(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<Vec<String>>, AuthError> {
    let rows = sqlx::query_scalar!(
        r#"select r.permissions
           from backend_user_roles ur
           join backend_roles r on r.id = ur.backend_role_id
           where ur.backend_user_id = $1"#,
        user_id
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Loads a user's per-permission overrides: a map of permission code to `1`
/// (allow) or `-1` (deny). A code that is absent inherits the role decision.
pub async fn load_user_permission_overrides(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<std::collections::HashMap<String, i64>, AuthError> {
    let value: Option<serde_json::Value> = sqlx::query_scalar!(
        "select permissions from backend_users where id = $1",
        user_id
    )
    .fetch_optional(pool)
    .await?;
    let overrides = value
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(overrides)
}

/// Replaces a user's per-permission overrides. Values other than `1` and `-1`
/// should not be stored; callers drop `inherit` entries before saving.
pub async fn set_user_permissions(
    pool: &PgPool,
    user_id: Uuid,
    overrides: &std::collections::HashMap<String, i64>,
) -> Result<(), AuthError> {
    let value = serde_json::to_value(overrides).unwrap_or_else(|_| serde_json::json!({}));
    sqlx::query!(
        "update backend_users set permissions = $1 where id = $2",
        value,
        user_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_session(
    pool: &PgPool,
    token_hash: &str,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<(), AuthError> {
    sqlx::query!(
        r#"insert into backend_sessions (token_hash, backend_user_id, expires_at)
           values ($1, $2, $3)"#,
        token_hash,
        user_id,
        expires_at
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns the owning user id of a non-expired session, if any.
pub async fn find_valid_session(
    pool: &PgPool,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, AuthError> {
    let user_id = sqlx::query_scalar!(
        r#"select backend_user_id from backend_sessions
           where token_hash = $1 and expires_at > $2"#,
        token_hash,
        now
    )
    .fetch_optional(pool)
    .await?;
    Ok(user_id)
}

pub async fn touch_session(
    pool: &PgPool,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    sqlx::query!(
        "update backend_sessions set last_seen_at = $2 where token_hash = $1",
        token_hash,
        now
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_session(pool: &PgPool, token_hash: &str) -> Result<(), AuthError> {
    sqlx::query!(
        "delete from backend_sessions where token_hash = $1",
        token_hash
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_access_log(
    pool: &PgPool,
    user_id: Option<Uuid>,
    username_attempted: &str,
    event: AccessEvent,
    ip_address: Option<&str>,
    user_agent: Option<&str>,
) -> Result<(), AuthError> {
    sqlx::query!(
        r#"insert into backend_access_log
               (backend_user_id, username_attempted, event, ip_address, user_agent)
           values ($1, $2, $3, $4, $5)"#,
        user_id,
        username_attempted,
        event.as_str(),
        ip_address,
        user_agent
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Counts recent failed login attempts for a username, for throttling.
pub async fn count_recent_failures(
    pool: &PgPool,
    username: &str,
    since: DateTime<Utc>,
) -> Result<i64, AuthError> {
    let count = sqlx::query_scalar!(
        r#"select count(*) as "count!"
           from backend_access_log
           where username_attempted = $1 and event = $2 and created_at >= $3"#,
        username,
        AccessEvent::LoginFailure.as_str(),
        since
    )
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Creates a backend user, returning its id. Intended for seeding the first
/// operator and for tests; ordinary user management goes through the admin.
#[allow(clippy::too_many_arguments)]
pub async fn create_user(
    pool: &PgPool,
    username: &str,
    email: &str,
    first_name: &str,
    last_name: Option<&str>,
    password_hash: &str,
    is_superuser: bool,
) -> Result<Uuid, AuthError> {
    let id = sqlx::query_scalar!(
        r#"insert into backend_users
               (username, email, first_name, last_name, password_hash, is_superuser)
           values ($1, $2, $3, $4, $5, $6)
           returning id"#,
        username,
        email,
        first_name,
        last_name,
        password_hash,
        is_superuser
    )
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Whether any backend user exists. Used to decide between the first-run setup
/// screen and the login screen.
pub async fn any_user_exists(pool: &PgPool) -> Result<bool, AuthError> {
    let exists = sqlx::query_scalar!(r#"select exists(select 1 from backend_users) as "exists!""#)
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

/// Sets an operator's own display timezone, or clears it with `None` so the
/// operator falls back to the deployment default. The IANA name is validated by
/// the caller before it reaches here.
pub async fn set_user_timezone(
    pool: &PgPool,
    user_id: Uuid,
    timezone: Option<&str>,
) -> Result<(), AuthError> {
    sqlx::query!(
        r#"update backend_users
           set timezone = $2, updated_at = now()
           where id = $1"#,
        user_id,
        timezone
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn create_role(
    pool: &PgPool,
    code: &str,
    name: &str,
    permissions: &[String],
) -> Result<Uuid, AuthError> {
    let id = sqlx::query_scalar!(
        r#"insert into backend_roles (code, name, permissions)
           values ($1, $2, $3)
           returning id"#,
        code,
        name,
        permissions
    )
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn assign_role(pool: &PgPool, user_id: Uuid, role_id: Uuid) -> Result<(), AuthError> {
    sqlx::query!(
        r#"insert into backend_user_roles (backend_user_id, backend_role_id)
           values ($1, $2)
           on conflict do nothing"#,
        user_id,
        role_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Lists backend users for admin tooling, ordered by creation time.
pub async fn list_backend_users(pool: &PgPool) -> Result<Vec<BackendUserSummary>, AuthError> {
    let users = sqlx::query_as!(
        BackendUserSummary,
        r#"select id, username, email, first_name, last_name,
                  is_superuser, is_active, created_at
           from backend_users
           order by created_at"#
    )
    .fetch_all(pool)
    .await?;
    Ok(users)
}

/// Sets a new password hash for a user by username, returning the number of rows
/// affected (0 means no such user). Deliberately does not depend on email.
pub async fn update_password_by_username(
    pool: &PgPool,
    username: &str,
    password_hash: &str,
) -> Result<u64, AuthError> {
    let result = sqlx::query!(
        r#"update backend_users
           set password_hash = $1, updated_at = now()
           where username = $2"#,
        password_hash,
        username
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Clears a user's failed-login records, releasing a lockout. Returns the number
/// of records removed.
pub async fn clear_failed_attempts(pool: &PgPool, username: &str) -> Result<u64, AuthError> {
    let result = sqlx::query!(
        r#"delete from backend_access_log
           where username_attempted = $1 and event = $2"#,
        username,
        AccessEvent::LoginFailure.as_str()
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
