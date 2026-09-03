//! Data access for the auth schema.
//!
//! Free functions over a [`Db`] (pool plus backend). Queries are built with
//! `sea-query` and bound through `laterite_core::query`, so they run on any
//! supported backend. Portability at the boundary: ids are `bigint`
//! auto-increment keys (the database assigns them, read back through
//! [`laterite_core::query::insert_returning_id`]), timestamps are stored as text
//! and converted to `DateTime<Utc>` here, and permission collections are stored
//! as JSON text.

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, Utc};
use laterite_core::query::{
    bind_values, bind_values_as, build, insert_returning_id, on_conflict_ignore,
};
use laterite_core::{AnyRowExt, Db};
use sea_query::{Expr, Order, Query};
use sqlx::any::AnyRow;
use sqlx::Row;

use crate::error::AuthError;
use crate::models::{AccessEvent, BackendUser, BackendUserSummary};
use crate::schema::{
    BackendAccessLog, BackendRoles, BackendSessions, BackendUserRoles, BackendUsers,
};

fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn ts(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>, AuthError> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| AuthError::Data(format!("timestamp `{s}`: {e}")))
}

/// Normalises a user-facing key (username or email) so lookups, uniqueness, and
/// login throttling behave identically on every backend. MySQL's default
/// collation is case-insensitive (and trailing-space-insensitive) while Postgres
/// and SQLite are case-sensitive, so the framework lower-cases and trims these
/// keys before storing or matching. Apply it on every write and lookup of a
/// username or email so `Root` and `root ` resolve to the same account anywhere.
fn normalize_key(s: &str) -> String {
    s.trim().to_lowercase()
}

fn user_from_row(row: &AnyRow) -> Result<BackendUser, AuthError> {
    Ok(BackendUser {
        id: row.try_get::<i64, _>("id")?,
        username: row.get_text("username")?,
        email: row.get_text("email")?,
        first_name: row.get_text("first_name")?,
        last_name: row.get_text_opt("last_name")?,
        password_hash: row.get_text("password_hash")?,
        is_superuser: row.get_bool("is_superuser")?,
        is_active: row.get_bool("is_active")?,
        timezone: row.get_text_opt("timezone")?,
        locale: row.get_text_opt("locale")?,
        created_at: parse_ts(&row.get_text("created_at")?)?,
        updated_at: parse_ts(&row.get_text("updated_at")?)?,
    })
}

fn summary_from_row(row: &AnyRow) -> Result<BackendUserSummary, AuthError> {
    Ok(BackendUserSummary {
        id: row.try_get::<i64, _>("id")?,
        username: row.get_text("username")?,
        email: row.get_text("email")?,
        first_name: row.get_text("first_name")?,
        last_name: row.get_text_opt("last_name")?,
        is_superuser: row.get_bool("is_superuser")?,
        is_active: row.get_bool("is_active")?,
        created_at: parse_ts(&row.get_text("created_at")?)?,
    })
}

const USER_COLS: [BackendUsers; 12] = [
    BackendUsers::Id,
    BackendUsers::Username,
    BackendUsers::Email,
    BackendUsers::FirstName,
    BackendUsers::LastName,
    BackendUsers::PasswordHash,
    BackendUsers::IsSuperuser,
    BackendUsers::IsActive,
    BackendUsers::Timezone,
    BackendUsers::Locale,
    BackendUsers::CreatedAt,
    BackendUsers::UpdatedAt,
];

/// Looks up a user by username without filtering on active state.
pub async fn find_user_by_username(
    db: &Db,
    username: &str,
) -> Result<Option<BackendUser>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(USER_COLS)
            .from(BackendUsers::Table)
            .and_where(Expr::col(BackendUsers::Username).eq(normalize_key(username)))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.map(|r| user_from_row(&r)).transpose()
}

/// Looks up an active user by id, used when resolving a session to an identity.
pub async fn find_active_user_by_id(db: &Db, id: i64) -> Result<Option<BackendUser>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(USER_COLS)
            .from(BackendUsers::Table)
            .and_where(Expr::col(BackendUsers::Id).eq(id))
            .and_where(Expr::col(BackendUsers::IsActive).eq(true))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.map(|r| user_from_row(&r)).transpose()
}

/// Returns the permission lists of every role assigned to a user (each stored
/// as a JSON array), for the service to flatten into a permission set.
pub async fn load_role_permissions(db: &Db, user_id: i64) -> Result<Vec<Vec<String>>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column((BackendRoles::Table, BackendRoles::Permissions))
            .from(BackendUserRoles::Table)
            .inner_join(
                BackendRoles::Table,
                Expr::col((BackendRoles::Table, BackendRoles::Id))
                    .equals((BackendUserRoles::Table, BackendUserRoles::BackendRoleId)),
            )
            .and_where(
                Expr::col((BackendUserRoles::Table, BackendUserRoles::BackendUserId)).eq(user_id),
            )
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let json = row.get_text("permissions")?;
        let perms: Vec<String> = serde_json::from_str(&json)
            .map_err(|e| AuthError::Data(format!("role permissions: {e}")))?;
        out.push(perms);
    }
    Ok(out)
}

/// Loads a user's per-permission overrides: a map of permission code to `1`
/// (allow) or `-1` (deny).
pub async fn load_user_permission_overrides(
    db: &Db,
    user_id: i64,
) -> Result<HashMap<String, i64>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(BackendUsers::Permissions)
            .from(BackendUsers::Table)
            .and_where(Expr::col(BackendUsers::Id).eq(user_id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    let overrides = match row {
        Some(r) => {
            let json = r.get_text("permissions")?;
            serde_json::from_str(&json).unwrap_or_default()
        }
        None => HashMap::new(),
    };
    Ok(overrides)
}

/// Replaces a user's per-permission overrides (stored as a JSON object).
pub async fn set_user_permissions(
    db: &Db,
    user_id: i64,
    overrides: &HashMap<String, i64>,
) -> Result<(), AuthError> {
    let json = serde_json::to_string(overrides).unwrap_or_else(|_| "{}".to_string());
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendUsers::Table)
            .value(BackendUsers::Permissions, json)
            .and_where(Expr::col(BackendUsers::Id).eq(user_id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn insert_session(
    db: &Db,
    token_hash: &str,
    user_id: i64,
    expires_at: DateTime<Utc>,
) -> Result<(), AuthError> {
    let now = now_ts();
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(BackendSessions::Table)
            .columns([
                BackendSessions::TokenHash,
                BackendSessions::BackendUserId,
                BackendSessions::CreatedAt,
                BackendSessions::LastSeenAt,
                BackendSessions::ExpiresAt,
            ])
            .values_panic([
                token_hash.into(),
                user_id.into(),
                now.clone().into(),
                now.into(),
                ts(expires_at).into(),
            ])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// A resolved non-expired session: its owning user and opaque data blob.
pub struct ValidSession {
    pub user_id: i64,
    pub data: Option<String>,
}

/// Returns a non-expired session (owner id plus its data blob), if any. The
/// blob is read in the same query, so exposing session state costs no extra
/// round-trip.
pub async fn find_valid_session(
    db: &Db,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<ValidSession>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([BackendSessions::BackendUserId, BackendSessions::Data])
            .from(BackendSessions::Table)
            .and_where(Expr::col(BackendSessions::TokenHash).eq(token_hash))
            .and_where(Expr::col(BackendSessions::ExpiresAt).gt(ts(now)))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    match row {
        Some(r) => Ok(Some(ValidSession {
            user_id: r.try_get::<i64, _>("backend_user_id")?,
            data: r.get_text_opt("data")?,
        })),
        None => Ok(None),
    }
}

/// Overwrites a session's opaque data blob. Callers write only when the blob
/// changed, so an unchanged request adds no write.
pub async fn set_session_data(db: &Db, token_hash: &str, data: &str) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendSessions::Table)
            .value(BackendSessions::Data, data)
            .and_where(Expr::col(BackendSessions::TokenHash).eq(token_hash))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn touch_session(db: &Db, token_hash: &str, now: DateTime<Utc>) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendSessions::Table)
            .value(BackendSessions::LastSeenAt, ts(now))
            .and_where(Expr::col(BackendSessions::TokenHash).eq(token_hash))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn delete_session(db: &Db, token_hash: &str) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendSessions::Table)
            .and_where(Expr::col(BackendSessions::TokenHash).eq(token_hash))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn insert_access_log(
    db: &Db,
    user_id: Option<i64>,
    username_attempted: &str,
    event: AccessEvent,
    ip_address: Option<&str>,
    user_agent: Option<&str>,
) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(BackendAccessLog::Table)
            .columns([
                BackendAccessLog::BackendUserId,
                BackendAccessLog::UsernameAttempted,
                BackendAccessLog::Event,
                BackendAccessLog::IpAddress,
                BackendAccessLog::UserAgent,
                BackendAccessLog::CreatedAt,
            ])
            .values_panic([
                user_id.into(),
                normalize_key(username_attempted).into(),
                event.as_str().into(),
                ip_address.map(str::to_string).into(),
                user_agent.map(str::to_string).into(),
                now_ts().into(),
            ])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Counts recent failed login attempts for a username, for throttling.
pub async fn count_recent_failures(
    db: &Db,
    username: &str,
    since: DateTime<Utc>,
) -> Result<i64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr(Expr::col(BackendAccessLog::Id).count())
            .from(BackendAccessLog::Table)
            .and_where(Expr::col(BackendAccessLog::UsernameAttempted).eq(normalize_key(username)))
            .and_where(Expr::col(BackendAccessLog::Event).eq(AccessEvent::LoginFailure.as_str()))
            .and_where(Expr::col(BackendAccessLog::CreatedAt).gte(ts(since)))
            .to_owned(),
    );
    let count: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&sql), values)
        .fetch_one(&db.pool)
        .await?
        .0;
    Ok(count)
}

/// Creates a backend user, returning the id the database assigned. Timestamps
/// are generated here (no database-side defaults) so the insert is portable.
#[allow(clippy::too_many_arguments)]
pub async fn create_user(
    db: &Db,
    username: &str,
    email: &str,
    first_name: &str,
    last_name: Option<&str>,
    password_hash: &str,
    is_superuser: bool,
) -> Result<i64, AuthError> {
    let now = now_ts();
    let stmt = Query::insert()
        .into_table(BackendUsers::Table)
        .columns([
            BackendUsers::Username,
            BackendUsers::Email,
            BackendUsers::FirstName,
            BackendUsers::LastName,
            BackendUsers::PasswordHash,
            BackendUsers::IsSuperuser,
            BackendUsers::CreatedAt,
            BackendUsers::UpdatedAt,
        ])
        .values_panic([
            normalize_key(username).into(),
            normalize_key(email).into(),
            first_name.into(),
            last_name.map(str::to_string).into(),
            password_hash.into(),
            is_superuser.into(),
            now.clone().into(),
            now.into(),
        ])
        .to_owned();
    Ok(insert_returning_id(db, stmt, BackendUsers::Id).await?)
}

/// Whether any backend user exists.
pub async fn any_user_exists(db: &Db) -> Result<bool, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr(Expr::col(BackendUsers::Id).count())
            .from(BackendUsers::Table)
            .to_owned(),
    );
    let count: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&sql), values)
        .fetch_one(&db.pool)
        .await?
        .0;
    Ok(count > 0)
}

/// Sets an operator's own display timezone, or clears it with `None`.
pub async fn set_user_timezone(
    db: &Db,
    user_id: i64,
    timezone: Option<&str>,
) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendUsers::Table)
            .value(BackendUsers::Timezone, timezone.map(str::to_string))
            .value(BackendUsers::UpdatedAt, now_ts())
            .and_where(Expr::col(BackendUsers::Id).eq(user_id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Sets an operator's own UI locale, or clears it with `None`.
pub async fn set_user_locale(db: &Db, user_id: i64, locale: Option<&str>) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendUsers::Table)
            .value(BackendUsers::Locale, locale.map(str::to_string))
            .value(BackendUsers::UpdatedAt, now_ts())
            .and_where(Expr::col(BackendUsers::Id).eq(user_id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub async fn create_role(
    db: &Db,
    code: &str,
    name: &str,
    permissions: &[String],
) -> Result<i64, AuthError> {
    let perms = serde_json::to_string(permissions).unwrap_or_else(|_| "[]".to_string());
    let stmt = Query::insert()
        .into_table(BackendRoles::Table)
        .columns([
            BackendRoles::Code,
            BackendRoles::Name,
            BackendRoles::Permissions,
            BackendRoles::CreatedAt,
        ])
        .values_panic([code.into(), name.into(), perms.into(), now_ts().into()])
        .to_owned();
    Ok(insert_returning_id(db, stmt, BackendRoles::Id).await?)
}

pub async fn assign_role(db: &Db, user_id: i64, role_id: i64) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(BackendUserRoles::Table)
            .columns([
                BackendUserRoles::BackendUserId,
                BackendUserRoles::BackendRoleId,
            ])
            .values_panic([user_id.into(), role_id.into()])
            .on_conflict(on_conflict_ignore([
                BackendUserRoles::BackendUserId,
                BackendUserRoles::BackendRoleId,
            ]))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Lists backend users for admin tooling, ordered by creation time.
pub async fn list_backend_users(db: &Db) -> Result<Vec<BackendUserSummary>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                BackendUsers::Id,
                BackendUsers::Username,
                BackendUsers::Email,
                BackendUsers::FirstName,
                BackendUsers::LastName,
                BackendUsers::IsSuperuser,
                BackendUsers::IsActive,
                BackendUsers::CreatedAt,
            ])
            .from(BackendUsers::Table)
            .order_by(BackendUsers::CreatedAt, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(summary_from_row).collect()
}

/// Sets a new password hash for a user by username, returning rows affected.
pub async fn update_password_by_username(
    db: &Db,
    username: &str,
    password_hash: &str,
) -> Result<u64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendUsers::Table)
            .value(BackendUsers::PasswordHash, password_hash)
            .value(BackendUsers::UpdatedAt, now_ts())
            .and_where(Expr::col(BackendUsers::Username).eq(normalize_key(username)))
            .to_owned(),
    );
    let result = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(result.rows_affected())
}

/// Clears a user's failed-login records, releasing a lockout.
pub async fn clear_failed_attempts(db: &Db, username: &str) -> Result<u64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendAccessLog::Table)
            .and_where(Expr::col(BackendAccessLog::UsernameAttempted).eq(normalize_key(username)))
            .and_where(Expr::col(BackendAccessLog::Event).eq(AccessEvent::LoginFailure.as_str()))
            .to_owned(),
    );
    let result = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(result.rows_affected())
}
