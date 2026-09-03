//! The authentication and authorization service.
//!
//! Composes the store, password, and permission layers into the flows the
//! admin surface calls: `authenticate` (throttle, verify, issue session, log),
//! `verify_session` (resolve a token to an identity), and `logout`.

use std::fmt::Write as _;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use laterite_core::Db;

use crate::error::AuthError;
use crate::models::{AccessEvent, BackendUser};
use crate::password;
use crate::permission::PermissionSet;
use crate::store;

/// Tunable auth policy. Loadable from a config section (all keys optional; an unset
/// key keeps its default):
///
/// ```toml
/// [auth]
/// session_ttl_secs = 43200
/// max_failures = 5
/// failure_window_secs = 900
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// How long an issued session remains valid.
    #[serde(rename = "session_ttl_secs", deserialize_with = "de_secs")]
    pub session_ttl: Duration,
    /// Failed attempts within `failure_window` before a username is locked out.
    pub max_failures: i64,
    /// The window over which failed attempts are counted.
    #[serde(rename = "failure_window_secs", deserialize_with = "de_secs")]
    pub failure_window: Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_ttl: Duration::from_secs(60 * 60 * 12),
            max_failures: 5,
            failure_window: Duration::from_secs(60 * 15),
        }
    }
}

/// Deserializes a whole-second count into a `Duration`.
fn de_secs<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
    Ok(Duration::from_secs(u64::deserialize(de)?))
}

#[cfg(test)]
mod auth_config_tests {
    use super::AuthConfig;
    use std::time::Duration;

    #[test]
    fn unset_keys_keep_defaults() {
        let cfg: AuthConfig = serde_json::from_str(r#"{"max_failures": 3}"#).unwrap();
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.session_ttl, Duration::from_secs(60 * 60 * 12));
        assert_eq!(cfg.failure_window, Duration::from_secs(60 * 15));
    }

    #[test]
    fn seconds_map_to_durations() {
        let cfg: AuthConfig = serde_json::from_str(
            r#"{"session_ttl_secs": 3600, "max_failures": 7, "failure_window_secs": 120}"#,
        )
        .unwrap();
        assert_eq!(cfg.session_ttl, Duration::from_secs(3600));
        assert_eq!(cfg.max_failures, 7);
        assert_eq!(cfg.failure_window, Duration::from_secs(120));
    }
}

/// Per-request context recorded in the access log.
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

/// A freshly issued session. `token` is the raw bearer value for the client
/// cookie; only its hash is persisted.
#[derive(Debug, Clone)]
pub struct IssuedSession {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// An authenticated backend user together with the permissions in force.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user: BackendUser,
    pub permissions: PermissionSet,
}

/// A resolved live session: the authenticated identity plus the opaque data
/// blob the surface stored on it (`None` until the surface writes one). Auth
/// does not interpret the blob.
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub identity: AuthenticatedUser,
    pub data: Option<String>,
}

impl AuthenticatedUser {
    /// Whether this identity holds `permission`.
    pub fn allows(&self, permission: &str) -> bool {
        self.permissions.allows(permission)
    }

    /// Returns an error unless this identity holds `permission`.
    pub fn require(&self, permission: &str) -> Result<(), AuthError> {
        if self.allows(permission) {
            Ok(())
        } else {
            Err(AuthError::PermissionDenied(permission.to_string()))
        }
    }
}

/// The details for creating a backend operator. Used by the CLI and the
/// first-run setup screen so account creation has one code path.
#[derive(Debug, Clone)]
pub struct NewOperator<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub first_name: &'a str,
    pub last_name: Option<&'a str>,
    pub password: &'a str,
    /// The operator's display timezone (an IANA name), or `None` to inherit the
    /// deployment default.
    pub timezone: Option<&'a str>,
}

/// The auth service. Cheap to clone: it holds a database handle and config.
#[derive(Clone)]
pub struct AuthService {
    db: Db,
    config: AuthConfig,
}

impl AuthService {
    pub fn new(db: Db, config: AuthConfig) -> Self {
        Self { db, config }
    }

    /// Verifies a username and password, and on success issues a session.
    ///
    /// Failures are throttled per username and every outcome is logged. The
    /// error deliberately does not reveal whether the username exists.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
        ctx: &RequestContext,
    ) -> Result<IssuedSession, AuthError> {
        let now = Utc::now();
        let since = now - chrono_from_std(self.config.failure_window);

        if store::count_recent_failures(&self.db, username, since).await?
            >= self.config.max_failures
        {
            self.log(None, username, AccessEvent::LockedOut, ctx)
                .await?;
            return Err(AuthError::TooManyAttempts);
        }

        let user = match store::find_user_by_username(&self.db, username).await? {
            Some(user) => user,
            None => {
                self.log(None, username, AccessEvent::LoginFailure, ctx)
                    .await?;
                return Err(AuthError::InvalidCredentials);
            }
        };

        if !password::verify_password(password, &user.password_hash)? {
            self.log(Some(user.id), username, AccessEvent::LoginFailure, ctx)
                .await?;
            return Err(AuthError::InvalidCredentials);
        }

        if !user.is_active {
            self.log(Some(user.id), username, AccessEvent::LoginFailure, ctx)
                .await?;
            return Err(AuthError::InactiveAccount);
        }

        let token = generate_token();
        let expires_at = now + chrono_from_std(self.config.session_ttl);
        store::insert_session(&self.db, &hash_token(&token), user.id, expires_at).await?;
        self.log(Some(user.id), username, AccessEvent::LoginSuccess, ctx)
            .await?;

        Ok(IssuedSession { token, expires_at })
    }

    /// Resolves a raw session token to an identity and its stored data blob,
    /// refreshing last-seen. Expired sessions, and sessions whose user was
    /// disabled or removed, resolve to [`AuthError::SessionInvalid`]. The blob
    /// is read in the same query as the session, adding no round-trip.
    pub async fn resolve_session(&self, token: &str) -> Result<ResolvedSession, AuthError> {
        let token_hash = hash_token(token);
        let now = Utc::now();

        let session = store::find_valid_session(&self.db, &token_hash, now)
            .await?
            .ok_or(AuthError::SessionInvalid)?;
        let user = store::find_active_user_by_id(&self.db, session.user_id)
            .await?
            .ok_or(AuthError::SessionInvalid)?;
        store::touch_session(&self.db, &token_hash, now).await?;

        let grants = store::load_role_permissions(&self.db, user.id)
            .await?
            .into_iter()
            .flatten();
        // Split the user's per-permission overrides into allow (1) and deny (-1),
        // which take precedence over the role grants.
        let overrides = store::load_user_permission_overrides(&self.db, user.id).await?;
        let (mut allow, mut deny) = (Vec::new(), Vec::new());
        for (code, decision) in overrides {
            match decision.signum() {
                1 => allow.push(code),
                -1 => deny.push(code),
                _ => {}
            }
        }
        let permissions = PermissionSet::with_overrides(user.is_superuser, grants, allow, deny);

        Ok(ResolvedSession {
            identity: AuthenticatedUser { user, permissions },
            data: session.data,
        })
    }

    /// Resolves a raw session token to an identity. A thin wrapper over
    /// [`AuthService::resolve_session`] for callers that need only the identity.
    pub async fn verify_session(&self, token: &str) -> Result<AuthenticatedUser, AuthError> {
        Ok(self.resolve_session(token).await?.identity)
    }

    /// Overwrites the opaque per-session data blob (the surface's serialised
    /// state, e.g. CSRF token + flash). The surface writes only when its blob
    /// changed, so an unchanged request adds no write.
    pub async fn set_session_data(&self, token: &str, data: &str) -> Result<(), AuthError> {
        store::set_session_data(&self.db, &hash_token(token), data).await
    }

    /// Invalidates a session. Unknown tokens are a no-op.
    pub async fn logout(&self, token: &str) -> Result<(), AuthError> {
        store::delete_session(&self.db, &hash_token(token)).await
    }

    /// Persists an operator's own display timezone. `Some(name)` sets an IANA
    /// timezone; `None` clears it so the operator falls back to the deployment
    /// default. Validating that `name` is a real timezone is the caller's job.
    pub async fn set_user_timezone(
        &self,
        user_id: i64,
        timezone: Option<&str>,
    ) -> Result<(), AuthError> {
        store::set_user_timezone(&self.db, user_id, timezone).await
    }

    /// Persists an operator's own UI locale. `Some(tag)` sets a base language tag;
    /// `None` clears it so the operator falls back to the deployment default.
    /// Validating that `tag` is a supported locale is the caller's job.
    pub async fn set_user_locale(
        &self,
        user_id: i64,
        locale: Option<&str>,
    ) -> Result<(), AuthError> {
        store::set_user_locale(&self.db, user_id, locale).await
    }

    /// Loads a user's per-permission overrides (code to `1` allow or `-1` deny).
    pub async fn user_permission_overrides(
        &self,
        user_id: i64,
    ) -> Result<std::collections::HashMap<String, i64>, AuthError> {
        store::load_user_permission_overrides(&self.db, user_id).await
    }

    /// Replaces a user's per-permission overrides. Callers pass only `1` and `-1`
    /// entries; an inherited permission is represented by its absence.
    pub async fn set_user_permissions(
        &self,
        user_id: i64,
        overrides: &std::collections::HashMap<String, i64>,
    ) -> Result<(), AuthError> {
        store::set_user_permissions(&self.db, user_id, overrides).await
    }

    /// Whether any backend operator exists yet. A fresh install with none is
    /// routed to first-run setup instead of login.
    pub async fn has_any_operator(&self) -> Result<bool, AuthError> {
        store::any_user_exists(&self.db).await
    }

    /// Creates a superuser operator: hashes the password, inserts the user, and
    /// records their timezone preference. The single account-creation path,
    /// shared by the CLI and the first-run setup screen.
    pub async fn create_superuser(&self, new: NewOperator<'_>) -> Result<i64, AuthError> {
        let hash = password::hash_password(new.password)?;
        let id = store::create_user(
            &self.db,
            new.username,
            new.email,
            new.first_name,
            new.last_name,
            &hash,
            true,
        )
        .await?;
        if new.timezone.is_some() {
            store::set_user_timezone(&self.db, id, new.timezone).await?;
        }
        Ok(id)
    }

    async fn log(
        &self,
        user_id: Option<i64>,
        username: &str,
        event: AccessEvent,
        ctx: &RequestContext,
    ) -> Result<(), AuthError> {
        store::insert_access_log(
            &self.db,
            user_id,
            username,
            event,
            ctx.ip_address.as_deref(),
            ctx.user_agent.as_deref(),
        )
        .await
    }
}

/// Converts a small, in-range `std::time::Duration` to `chrono::Duration`.
/// The auth policy durations are hours at most, well within range.
fn chrono_from_std(d: Duration) -> chrono::Duration {
    chrono::Duration::from_std(d).expect("auth policy duration out of range")
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    to_hex(&bytes)
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    to_hex(&hasher.finalize())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_user(db: &Db, username: &str, password: &str, superuser: bool) -> i64 {
        let hash = password::hash_password(password).unwrap();
        store::create_user(
            db,
            username,
            &format!("{username}@example.test"),
            "Test",
            Some("Operator"),
            &hash,
            superuser,
        )
        .await
        .unwrap()
    }

    fn service(db: Db) -> AuthService {
        AuthService::new(db, AuthConfig::default())
    }

    /// A fresh test database with this module's migrations applied through the
    /// framework runner, on whichever backend the run targets (see
    /// `laterite_core::testing`). Hold the returned guard for the test's lifetime.
    async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
        laterite_core::testing::connect_test(&[crate::migrations()]).await
    }

    #[tokio::test]
    async fn authenticate_issues_a_verifiable_session() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        let session = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .expect("login should succeed");
        let identity = svc
            .verify_session(&session.token)
            .await
            .expect("session should resolve");

        assert_eq!(identity.user.username, "root");
        assert_eq!(identity.user.full_name(), "Test Operator");
        assert!(identity.allows("anything.superuser.can.do"));
    }

    #[tokio::test]
    async fn session_data_blob_round_trips() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        let session = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();

        // A fresh session has no blob.
        let resolved = svc.resolve_session(&session.token).await.unwrap();
        assert_eq!(resolved.data, None);
        assert_eq!(resolved.identity.user.username, "root");

        // The surface writes its opaque state; the next resolve reads it back.
        svc.set_session_data(&session.token, r#"{"v":1,"csrf":"abc"}"#)
            .await
            .unwrap();
        let resolved = svc.resolve_session(&session.token).await.unwrap();
        assert_eq!(resolved.data.as_deref(), Some(r#"{"v":1,"csrf":"abc"}"#));
    }

    #[tokio::test]
    async fn username_is_case_insensitive_across_backends() {
        // The account is created lower-cased, and a differently-cased login
        // resolves to it: this must hold identically on every backend (MySQL's
        // collation is case-insensitive, Postgres and SQLite are not).
        let (pool, _guard) = test_db().await;
        let hash = password::hash_password("pw").unwrap();
        store::create_user(
            &pool,
            "Root",
            "Root@Example.test",
            "Case",
            None,
            &hash,
            true,
        )
        .await
        .unwrap();
        let svc = service(pool);
        let session = svc
            .authenticate("ROOT", "pw", &RequestContext::default())
            .await
            .expect("case-varied login should resolve to the same account");
        let identity = svc.verify_session(&session.token).await.unwrap();
        assert_eq!(identity.user.username, "root");
        assert_eq!(identity.user.email, "root@example.test");
    }

    #[tokio::test]
    async fn full_name_falls_back_to_first_name_when_last_is_absent() {
        let (pool, _guard) = test_db().await;
        let hash = password::hash_password("pw").unwrap();
        store::create_user(
            &pool,
            "mono",
            "mono@example.test",
            "Prakash",
            None,
            &hash,
            true,
        )
        .await
        .unwrap();
        let svc = service(pool);
        let session = svc
            .authenticate("mono", "pw", &RequestContext::default())
            .await
            .unwrap();
        let identity = svc.verify_session(&session.token).await.unwrap();
        assert_eq!(identity.user.full_name(), "Prakash");
    }

    #[tokio::test]
    async fn operator_timezone_round_trips_and_clears() {
        let (pool, _guard) = test_db().await;
        let id = seed_user(&pool, "tz", "pw", true).await;

        // A fresh operator has no preference and inherits the default.
        let user = store::find_active_user_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.timezone, None);

        let svc = service(pool.clone());
        svc.set_user_timezone(id, Some("Asia/Kolkata"))
            .await
            .unwrap();
        let user = store::find_active_user_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.timezone.as_deref(), Some("Asia/Kolkata"));

        // Clearing it returns the operator to the default.
        svc.set_user_timezone(id, None).await.unwrap();
        let user = store::find_active_user_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.timezone, None);
    }

    #[tokio::test]
    async fn operator_locale_round_trips_and_clears() {
        let (pool, _guard) = test_db().await;
        let id = seed_user(&pool, "loc", "pw", true).await;

        // A fresh operator has no preference and inherits the default.
        let user = store::find_active_user_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.locale, None);

        let svc = service(pool.clone());
        svc.set_user_locale(id, Some("kn")).await.unwrap();
        let user = store::find_active_user_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.locale.as_deref(), Some("kn"));

        // Clearing it returns the operator to the default.
        svc.set_user_locale(id, None).await.unwrap();
        let user = store::find_active_user_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.locale, None);
    }

    #[tokio::test]
    async fn has_any_operator_flips_after_the_first_account() {
        let (pool, _guard) = test_db().await;
        let svc = service(pool);

        // A fresh install has no operators, so setup (not login) applies.
        assert!(!svc.has_any_operator().await.unwrap());

        svc.create_superuser(NewOperator {
            username: "first",
            email: "first@example.test",
            first_name: "First",
            last_name: None,
            password: "hunter2",
            timezone: Some("Asia/Kolkata"),
        })
        .await
        .unwrap();

        assert!(svc.has_any_operator().await.unwrap());

        // The account is a usable superuser with the onboarding timezone recorded.
        let session = svc
            .authenticate("first", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        let identity = svc.verify_session(&session.token).await.unwrap();
        assert!(identity.allows("anything.a.superuser.can.do"));
        assert_eq!(identity.user.timezone.as_deref(), Some("Asia/Kolkata"));
    }

    #[tokio::test]
    async fn wrong_password_is_rejected() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", false).await;
        let svc = service(pool);

        let err = svc
            .authenticate("root", "wrong", &RequestContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[tokio::test]
    async fn unknown_user_is_rejected_without_distinction() {
        let (pool, _guard) = test_db().await;
        let svc = service(pool);
        let err = svc
            .authenticate("ghost", "whatever", &RequestContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[tokio::test]
    async fn lockout_trips_after_max_failures() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", false).await;
        let svc = AuthService::new(
            pool,
            AuthConfig {
                max_failures: 3,
                ..AuthConfig::default()
            },
        );
        let ctx = RequestContext::default();

        for _ in 0..3 {
            let err = svc.authenticate("root", "bad", &ctx).await.unwrap_err();
            assert!(matches!(err, AuthError::InvalidCredentials));
        }
        // The correct password is now refused: the account is locked out.
        let err = svc.authenticate("root", "hunter2", &ctx).await.unwrap_err();
        assert!(matches!(err, AuthError::TooManyAttempts));
    }

    #[tokio::test]
    async fn permissions_come_from_assigned_roles() {
        let (pool, _guard) = test_db().await;
        let user_id = seed_user(&pool, "mod", "pw", false).await;
        let role_id = store::create_role(
            &pool,
            "content_editor",
            "Content Editor",
            &["posts.*".to_string()],
        )
        .await
        .unwrap();
        store::assign_role(&pool, user_id, role_id).await.unwrap();

        let svc = service(pool);
        let session = svc
            .authenticate("mod", "pw", &RequestContext::default())
            .await
            .unwrap();
        let identity = svc.verify_session(&session.token).await.unwrap();

        assert!(identity.allows("posts.approve"));
        assert!(!identity.allows("users.edit"));
        identity.require("posts.edit").unwrap();
        assert!(identity.require("users.edit").is_err());
    }

    #[tokio::test]
    async fn logout_invalidates_the_session() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "pw", true).await;
        let svc = service(pool);

        let session = svc
            .authenticate("root", "pw", &RequestContext::default())
            .await
            .unwrap();
        svc.verify_session(&session.token).await.unwrap();
        svc.logout(&session.token).await.unwrap();

        let err = svc.verify_session(&session.token).await.unwrap_err();
        assert!(matches!(err, AuthError::SessionInvalid));
    }

    #[tokio::test]
    async fn inactive_account_is_refused_after_correct_password() {
        let (pool, _guard) = test_db().await;
        let user_id = seed_user(&pool, "root", "pw", false).await;
        // Deactivate through the query layer so the placeholder renders per backend.
        let (sql, values) = laterite_core::query::build(
            pool.backend,
            sea_query::Query::update()
                .table(crate::schema::BackendUsers::Table)
                .value(crate::schema::BackendUsers::IsActive, false)
                .and_where(sea_query::Expr::col(crate::schema::BackendUsers::Id).eq(user_id))
                .to_owned(),
        );
        laterite_core::query::bind_values(sqlx::query(&sql), values)
            .execute(&pool.pool)
            .await
            .unwrap();

        let svc = service(pool);
        let err = svc
            .authenticate("root", "pw", &RequestContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InactiveAccount));
    }

    #[tokio::test]
    async fn reset_password_updates_the_hash() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "oldpw", true).await;

        let new_hash = password::hash_password("newpw").unwrap();
        let affected = store::update_password_by_username(&pool, "root", &new_hash)
            .await
            .unwrap();
        assert_eq!(affected, 1);

        let svc = service(pool);
        let ctx = RequestContext::default();
        assert!(matches!(
            svc.authenticate("root", "oldpw", &ctx).await.unwrap_err(),
            AuthError::InvalidCredentials
        ));
        svc.authenticate("root", "newpw", &ctx).await.unwrap();
    }

    #[tokio::test]
    async fn reset_password_reports_unknown_user() {
        let (pool, _guard) = test_db().await;
        let hash = password::hash_password("x").unwrap();
        let affected = store::update_password_by_username(&pool, "ghost", &hash)
            .await
            .unwrap();
        assert_eq!(affected, 0);
    }

    #[tokio::test]
    async fn list_users_returns_all_seeded() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "alice", "pw", true).await;
        seed_user(&pool, "bob", "pw", false).await;

        let users = store::list_backend_users(&pool).await.unwrap();
        assert_eq!(users.len(), 2);
        assert!(users
            .iter()
            .any(|u| u.username == "alice" && u.is_superuser));
        assert!(users.iter().any(|u| u.username == "bob" && !u.is_superuser));
    }

    #[tokio::test]
    async fn unlock_clears_the_lockout() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "pw", false).await;
        let svc = AuthService::new(
            pool.clone(),
            AuthConfig {
                max_failures: 3,
                ..AuthConfig::default()
            },
        );
        let ctx = RequestContext::default();

        for _ in 0..3 {
            let _ = svc.authenticate("root", "bad", &ctx).await;
        }
        assert!(matches!(
            svc.authenticate("root", "pw", &ctx).await.unwrap_err(),
            AuthError::TooManyAttempts
        ));

        let cleared = store::clear_failed_attempts(&pool, "root").await.unwrap();
        assert!(cleared >= 3);
        svc.authenticate("root", "pw", &ctx).await.unwrap();
    }
}
