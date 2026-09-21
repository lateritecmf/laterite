//! Data access for the auth schema.
//!
//! Free functions over a [`Db`] (pool plus backend). Queries are built with
//! `sea-query` and bound through `laterite_core::query`, so they run on any
//! supported backend. Portability at the boundary: ids are `bigint`
//! auto-increment keys (the database assigns them, read back through
//! [`laterite_core::query::insert_returning_id`]), timestamps are stored as text
//! and converted to `DateTime<Utc>` here, and permission collections are stored
//! as JSON text.
//!
//! Session rows are the exception to the module being public. Reading one is
//! what pushes its idle clock forward, so a caller that reached the row directly
//! would authenticate a request and leave the session ageing as though the
//! request never happened. The session functions are therefore crate-visible and
//! [`crate::AuthService`] is the only way in: the renewal cannot be skipped
//! because there is no path that skips it.

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
    BackendAccessLog, BackendAuditLog, BackendRememberTokens, BackendRoles, BackendSessions,
    BackendUserPreferences, BackendUserRoles, BackendUsers,
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
/// A user by id whatever their state. Distinct from
/// [`find_active_user_by_id`], which is the authentication path: this one is for
/// administering an account, where a deactivated one still has to be readable.
pub(crate) async fn find_user_by_id(db: &Db, id: i64) -> Result<Option<BackendUser>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(USER_COLS)
            .from(BackendUsers::Table)
            .and_where(Expr::col(BackendUsers::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.map(|r| user_from_row(&r)).transpose()
}

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

pub(crate) async fn insert_session(
    db: &Db,
    token_hash: &str,
    user_id: i64,
    expires_at: DateTime<Utc>,
    ip_address: Option<&str>,
    user_agent: Option<&str>,
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
                BackendSessions::IpAddress,
                BackendSessions::UserAgent,
            ])
            .values_panic([
                token_hash.into(),
                user_id.into(),
                now.clone().into(),
                now.into(),
                ts(expires_at).into(),
                ip_address.into(),
                user_agent.into(),
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
    /// When the session was issued. Fixes the absolute ceiling.
    pub created_at: DateTime<Utc>,
    /// Last request on this session. Fixes the idle deadline.
    pub last_seen_at: DateTime<Utc>,
}

/// Returns a non-expired session (owner id plus its data blob), if any. The
/// blob is read in the same query, so exposing session state costs no extra
/// round-trip.
pub(crate) async fn find_valid_session(
    db: &Db,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<ValidSession>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                BackendSessions::BackendUserId,
                BackendSessions::Data,
                BackendSessions::CreatedAt,
                BackendSessions::LastSeenAt,
            ])
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
            created_at: parse_ts(&r.get_text("created_at")?)?,
            last_seen_at: parse_ts(&r.get_text("last_seen_at")?)?,
        })),
        None => Ok(None),
    }
}

/// Overwrites a session's opaque data blob. Callers write only when the blob
/// changed, so an unchanged request adds no write.
pub(crate) async fn set_session_data(
    db: &Db,
    token_hash: &str,
    data: &str,
) -> Result<(), AuthError> {
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

/// Pushes a session's idle clock and its effective deadline out together. The
/// two are written in one statement so a reader can never see a refreshed
/// last-seen against a stale deadline.
pub(crate) async fn renew_session(
    db: &Db,
    token_hash: &str,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendSessions::Table)
            .value(BackendSessions::LastSeenAt, ts(now))
            .value(BackendSessions::ExpiresAt, ts(expires_at))
            .and_where(Expr::col(BackendSessions::TokenHash).eq(token_hash))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// A live "stay signed in" credential. `verifier_hash` is checked by the
/// caller; the row is found by selector alone so a wrong verifier still costs
/// one indexed lookup and nothing more.
pub(crate) struct RememberToken {
    pub user_id: i64,
    pub verifier_hash: String,
}

pub(crate) async fn insert_remember_token(
    db: &Db,
    selector: &str,
    verifier_hash: &str,
    user_id: i64,
    expires_at: DateTime<Utc>,
) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(BackendRememberTokens::Table)
            .columns([
                BackendRememberTokens::Selector,
                BackendRememberTokens::VerifierHash,
                BackendRememberTokens::BackendUserId,
                BackendRememberTokens::CreatedAt,
                BackendRememberTokens::ExpiresAt,
            ])
            .values_panic([
                selector.into(),
                verifier_hash.into(),
                user_id.into(),
                now_ts().into(),
                ts(expires_at).into(),
            ])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub(crate) async fn find_remember_token(
    db: &Db,
    selector: &str,
    now: DateTime<Utc>,
) -> Result<Option<RememberToken>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                BackendRememberTokens::BackendUserId,
                BackendRememberTokens::VerifierHash,
            ])
            .from(BackendRememberTokens::Table)
            .and_where(Expr::col(BackendRememberTokens::Selector).eq(selector))
            .and_where(Expr::col(BackendRememberTokens::ExpiresAt).gt(ts(now)))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    match row {
        Some(r) => Ok(Some(RememberToken {
            user_id: r.try_get::<i64, _>("backend_user_id")?,
            verifier_hash: r.get_text("verifier_hash")?,
        })),
        None => Ok(None),
    }
}

/// Swaps in a new secret for a credential, keeping its selector. The selector
/// is the series: holding it stable is what lets a stale secret be recognised
/// as a copy rather than mistaken for an unknown credential.
pub(crate) async fn rotate_remember_token(
    db: &Db,
    selector: &str,
    verifier_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendRememberTokens::Table)
            .value(BackendRememberTokens::VerifierHash, verifier_hash)
            .value(BackendRememberTokens::ExpiresAt, ts(expires_at))
            .and_where(Expr::col(BackendRememberTokens::Selector).eq(selector))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub(crate) async fn delete_remember_token(db: &Db, selector: &str) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendRememberTokens::Table)
            .and_where(Expr::col(BackendRememberTokens::Selector).eq(selector))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Drops every remember credential a user holds. The answer to a verifier
/// mismatch, which means a copy of a cookie is in circulation, and to a
/// deliberate "sign out everywhere".
pub(crate) async fn delete_user_remember_tokens(db: &Db, user_id: i64) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendRememberTokens::Table)
            .and_where(Expr::col(BackendRememberTokens::BackendUserId).eq(user_id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// One live session of a user, for the account's own sessions list.
pub(crate) struct SessionSummary {
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

pub(crate) async fn list_user_sessions(
    db: &Db,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<Vec<SessionSummary>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                BackendSessions::TokenHash,
                BackendSessions::CreatedAt,
                BackendSessions::LastSeenAt,
                BackendSessions::ExpiresAt,
                BackendSessions::IpAddress,
                BackendSessions::UserAgent,
            ])
            .from(BackendSessions::Table)
            .and_where(Expr::col(BackendSessions::BackendUserId).eq(user_id))
            .and_where(Expr::col(BackendSessions::ExpiresAt).gt(ts(now)))
            .order_by(BackendSessions::LastSeenAt, Order::Desc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok(SessionSummary {
                token_hash: r.get_text("token_hash")?,
                created_at: parse_ts(&r.get_text("created_at")?)?,
                last_seen_at: parse_ts(&r.get_text("last_seen_at")?)?,
                expires_at: parse_ts(&r.get_text("expires_at")?)?,
                ip_address: r.get_text_opt("ip_address")?,
                user_agent: r.get_text_opt("user_agent")?,
            })
        })
        .collect()
}

/// Deletes one of a user's sessions. Scoped by owner, so a forged id can only
/// ever end one of the caller's own sessions.
pub(crate) async fn delete_user_session(
    db: &Db,
    user_id: i64,
    token_hash: &str,
) -> Result<u64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendSessions::Table)
            .and_where(Expr::col(BackendSessions::BackendUserId).eq(user_id))
            .and_where(Expr::col(BackendSessions::TokenHash).eq(token_hash))
            .to_owned(),
    );
    let done = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(done.rows_affected())
}

/// Ends every session a user holds except the one they are asking from.
pub(crate) async fn delete_user_sessions_except(
    db: &Db,
    user_id: i64,
    keep: &str,
) -> Result<u64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendSessions::Table)
            .and_where(Expr::col(BackendSessions::BackendUserId).eq(user_id))
            .and_where(Expr::col(BackendSessions::TokenHash).ne(keep))
            .to_owned(),
    );
    let done = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(done.rows_affected())
}

pub(crate) async fn delete_session(db: &Db, token_hash: &str) -> Result<(), AuthError> {
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

/// One row of the audit log, newest-first from [`recent_audit`].
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub actor_user_id: Option<i64>,
    pub actor_username: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Appends one entry to the append-only audit log.
#[allow(clippy::too_many_arguments)]
pub async fn insert_audit_log(
    db: &Db,
    actor_user_id: Option<i64>,
    actor_username: &str,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    detail: Option<&str>,
) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(BackendAuditLog::Table)
            .columns([
                BackendAuditLog::ActorUserId,
                BackendAuditLog::ActorUsername,
                BackendAuditLog::Action,
                BackendAuditLog::TargetType,
                BackendAuditLog::TargetId,
                BackendAuditLog::Detail,
                BackendAuditLog::CreatedAt,
            ])
            .values_panic([
                actor_user_id.into(),
                actor_username.to_string().into(),
                action.to_string().into(),
                target_type.map(str::to_string).into(),
                target_id.map(str::to_string).into(),
                detail.map(str::to_string).into(),
                now_ts().into(),
            ])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// The most recent audit entries, newest first (id descending, matching insert
/// order). Feeds the admin audit view.
pub async fn recent_audit(db: &Db, limit: u64) -> Result<Vec<AuditRecord>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                BackendAuditLog::ActorUserId,
                BackendAuditLog::ActorUsername,
                BackendAuditLog::Action,
                BackendAuditLog::TargetType,
                BackendAuditLog::TargetId,
                BackendAuditLog::Detail,
                BackendAuditLog::CreatedAt,
            ])
            .from(BackendAuditLog::Table)
            .order_by(BackendAuditLog::Id, Order::Desc)
            .limit(limit)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(audit_from_row).collect()
}

fn audit_from_row(row: &AnyRow) -> Result<AuditRecord, AuthError> {
    Ok(AuditRecord {
        actor_user_id: row.get_int_opt("actor_user_id")?,
        actor_username: row.get_text("actor_username")?,
        action: row.get_text("action")?,
        target_type: row.get_text_opt("target_type")?,
        target_id: row.get_text_opt("target_id")?,
        detail: row.get_text_opt("detail")?,
        created_at: parse_ts(&row.get_text("created_at")?)?,
    })
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

/// A role the framework owns: its permissions come from the registry, not from
/// the database, so they cannot drift as modules add permissions.
pub struct SystemRole<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub permissions: Vec<String>,
}

/// Writes the system roles, creating what is missing and rewriting what exists.
///
/// Runs at every boot: a module that registers a new permission has it in the
/// right role the moment it loads, and an operator cannot leave a system role
/// holding a permission the registry no longer declares. Only the framework's
/// own rows are touched; a role an operator made is never rewritten.
pub async fn sync_system_roles(db: &Db, roles: &[SystemRole<'_>]) -> Result<(), AuthError> {
    for role in roles {
        let perms = serde_json::to_string(&role.permissions).unwrap_or_else(|_| "[]".to_string());
        // Insert-then-update rather than select-then-insert: two instances booting
        // together both reach the insert, and the loser is ignored instead of
        // failing the unique key and taking its boot down with it.
        let (sql, values) = build(
            db.backend,
            Query::insert()
                .into_table(BackendRoles::Table)
                .columns([
                    BackendRoles::Code,
                    BackendRoles::Name,
                    BackendRoles::Description,
                    BackendRoles::Permissions,
                    BackendRoles::IsSystem,
                    BackendRoles::CreatedAt,
                ])
                .values_panic([
                    role.code.into(),
                    role.name.into(),
                    role.description.into(),
                    perms.clone().into(),
                    true.into(),
                    now_ts().into(),
                ])
                .on_conflict(on_conflict_ignore([BackendRoles::Code]))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
        let (sql, values) = build(
            db.backend,
            Query::update()
                .table(BackendRoles::Table)
                .values([
                    (BackendRoles::Name, role.name.into()),
                    (BackendRoles::Description, role.description.into()),
                    (BackendRoles::Permissions, perms.into()),
                    (BackendRoles::IsSystem, true.into()),
                ])
                .and_where(Expr::col(BackendRoles::Code).eq(role.code))
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
    }
    Ok(())
}

/// The id of the role with this code, if there is one.
pub async fn role_id_by_code(db: &Db, code: &str) -> Result<Option<i64>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(BackendRoles::Id)
            .from(BackendRoles::Table)
            .and_where(Expr::col(BackendRoles::Code).eq(code))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(match row {
        Some(row) => Some(row.try_get::<i64, _>("id")?),
        None => None,
    })
}

/// The permission codes this role grants.
pub async fn role_permissions(db: &Db, id: i64) -> Result<Vec<String>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(BackendRoles::Permissions)
            .from(BackendRoles::Table)
            .and_where(Expr::col(BackendRoles::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(match row {
        Some(row) => serde_json::from_str(&row.get_text("permissions")?).unwrap_or_default(),
        None => Vec::new(),
    })
}

/// Whether this role is one the framework owns.
pub async fn role_is_system(db: &Db, id: i64) -> Result<bool, AuthError> {
    let (sql, values) = is_system_query(db.backend, id);
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    read_is_system(row)
}

/// The same question asked on an open transaction, for a listener that must see
/// the state the write is running against.
pub async fn role_is_system_on(
    conn: &mut sqlx::AnyConnection,
    backend: laterite_core::DbBackend,
    id: i64,
) -> Result<bool, AuthError> {
    let (sql, values) = is_system_query(backend, id);
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&mut *conn)
        .await?;
    read_is_system(row)
}

fn is_system_query(backend: laterite_core::DbBackend, id: i64) -> (String, sea_query::Values) {
    build(
        backend,
        Query::select()
            .column(BackendRoles::IsSystem)
            .from(BackendRoles::Table)
            .and_where(Expr::col(BackendRoles::Id).eq(id))
            .to_owned(),
    )
}

/// A missing row is not a system role: it is nothing at all.
fn read_is_system(row: Option<AnyRow>) -> Result<bool, AuthError> {
    Ok(match row {
        Some(row) => row.get_bool("is_system")?,
        None => false,
    })
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

/// Sets whether a user may sign in. Returns rows affected, so a caller can tell
/// a no-op from a missing user.
pub(crate) async fn set_user_active(db: &Db, user_id: i64, active: bool) -> Result<u64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(BackendUsers::Table)
            .value(BackendUsers::IsActive, active)
            .and_where(Expr::col(BackendUsers::Id).eq(user_id))
            .to_owned(),
    );
    let done = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(done.rows_affected())
}

/// How many active superusers there are besides `excluding`.
///
/// Deactivating the last one would leave a panel nobody can fully administer,
/// recoverable only from the command line, so the caller refuses it.
pub(crate) async fn other_active_superusers(db: &Db, excluding: i64) -> Result<i64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr(Expr::col(BackendUsers::Id).count())
            .from(BackendUsers::Table)
            .and_where(Expr::col(BackendUsers::IsSuperuser).eq(true))
            .and_where(Expr::col(BackendUsers::IsActive).eq(true))
            .and_where(Expr::col(BackendUsers::Id).ne(excluding))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await?;
    Ok(row.try_get::<i64, _>(0).unwrap_or(0))
}

/// Ends every session a user holds. Used when an account is deactivated: an
/// account that may not sign in must not stay signed in either.
pub(crate) async fn delete_all_user_sessions(db: &Db, user_id: i64) -> Result<u64, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendSessions::Table)
            .and_where(Expr::col(BackendSessions::BackendUserId).eq(user_id))
            .to_owned(),
    );
    let done = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(done.rows_affected())
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

/// One operator's stored choice for `key`, or `None` if they have not made one.
pub async fn user_preference(
    db: &Db,
    user_id: i64,
    key: &str,
) -> Result<Option<String>, AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(BackendUserPreferences::Value)
            .from(BackendUserPreferences::Table)
            .and_where(Expr::col(BackendUserPreferences::UserId).eq(user_id))
            .and_where(Expr::col(BackendUserPreferences::PreferenceKey).eq(key))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(row.and_then(|r| r.get_text_opt("value").ok().flatten()))
}

/// Stores one operator's choice for `key`, replacing any previous one.
///
/// Written as delete-then-insert rather than an upsert: the three backends spell
/// `ON CONFLICT` differently, and a preference write is rare and already inside
/// its own request, so the portable pair costs nothing worth optimising.
pub async fn set_user_preference(
    db: &Db,
    user_id: i64,
    key: &str,
    value: &str,
) -> Result<(), AuthError> {
    let mut tx = db.pool.begin().await?;
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendUserPreferences::Table)
            .and_where(Expr::col(BackendUserPreferences::UserId).eq(user_id))
            .and_where(Expr::col(BackendUserPreferences::PreferenceKey).eq(key))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&mut *tx)
        .await?;
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(BackendUserPreferences::Table)
            .columns([
                BackendUserPreferences::UserId,
                BackendUserPreferences::PreferenceKey,
                BackendUserPreferences::Value,
            ])
            .values_panic([
                user_id.into(),
                key.to_string().into(),
                value.to_string().into(),
            ])
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Removes one operator's choice for `key`, so they fall back to the default.
pub async fn clear_user_preference(db: &Db, user_id: i64, key: &str) -> Result<(), AuthError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(BackendUserPreferences::Table)
            .and_where(Expr::col(BackendUserPreferences::UserId).eq(user_id))
            .and_where(Expr::col(BackendUserPreferences::PreferenceKey).eq(key))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterite_core::testing::connect_test;

    async fn operator(db: &Db) -> i64 {
        let svc = crate::AuthService::new(db.clone(), crate::AuthConfig::default());
        svc.create_superuser(crate::NewOperator {
            username: "root",
            email: "root@acme.test",
            first_name: "Root",
            last_name: None,
            password: "rootpw12345",
            timezone: None,
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_preference_round_trips_and_replaces() {
        let (db, _guard) = connect_test(&[crate::migrations()]).await;
        let id = operator(&db).await;

        // Unset reads as absent, not as an empty value.
        assert_eq!(
            user_preference(&db, id, "list.columns./roles")
                .await
                .unwrap(),
            None
        );

        set_user_preference(&db, id, "list.columns./roles", "code,name")
            .await
            .unwrap();
        assert_eq!(
            user_preference(&db, id, "list.columns./roles")
                .await
                .unwrap(),
            Some("code,name".to_string())
        );

        // A second write replaces rather than accumulating, which the unique
        // index would otherwise refuse.
        set_user_preference(&db, id, "list.columns./roles", "code")
            .await
            .unwrap();
        assert_eq!(
            user_preference(&db, id, "list.columns./roles")
                .await
                .unwrap(),
            Some("code".to_string())
        );

        clear_user_preference(&db, id, "list.columns./roles")
            .await
            .unwrap();
        assert_eq!(
            user_preference(&db, id, "list.columns./roles")
                .await
                .unwrap(),
            None
        );
    }

    /// Preferences are per operator and per key: one does not leak into another.
    #[tokio::test]
    async fn preferences_are_scoped() {
        let (db, _guard) = connect_test(&[crate::migrations()]).await;
        let root = operator(&db).await;
        let hash = crate::password::hash_password("otherpw12345").unwrap();
        let other = create_user(&db, "other", "other@acme.test", "Other", None, &hash, false)
            .await
            .unwrap();

        set_user_preference(&db, root, "list.columns./roles", "code")
            .await
            .unwrap();
        assert_eq!(
            user_preference(&db, other, "list.columns./roles")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            user_preference(&db, root, "list.columns./users")
                .await
                .unwrap(),
            None
        );
    }
}
