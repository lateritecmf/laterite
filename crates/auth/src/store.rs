//! Data access for the auth schema.
//!
//! Free functions over a `PgPool`. Query text is checked against the database
//! at compile time by the sqlx macros; the service layer composes these into
//! the authentication and authorization flows.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AuthError;
use crate::models::{AccessEvent, BackendUser};

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
                  is_superuser, is_active, created_at, updated_at
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
                  is_superuser, is_active, created_at, updated_at
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
