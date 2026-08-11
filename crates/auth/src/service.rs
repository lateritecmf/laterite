//! The authentication and authorization service.
//!
//! Composes the store, password, and permission layers into the flows the
//! admin surface calls: `authenticate` (throttle, verify, issue session, log),
//! `verify_session` (resolve a token to an identity), and `logout`.

use std::fmt::Write as _;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::error::AuthError;
use crate::models::{AccessEvent, BackendUser};
use crate::password;
use crate::permission::PermissionSet;
use crate::store;

/// Tunable auth policy.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// How long an issued session remains valid.
    pub session_ttl: Duration,
    /// Failed attempts within `failure_window` before a username is locked out.
    pub max_failures: i64,
    /// The window over which failed attempts are counted.
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

/// The auth service. Cheap to clone: it holds a pool handle and config.
#[derive(Clone)]
pub struct AuthService {
    pool: PgPool,
    config: AuthConfig,
}

impl AuthService {
    pub fn new(pool: PgPool, config: AuthConfig) -> Self {
        Self { pool, config }
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

        if store::count_recent_failures(&self.pool, username, since).await?
            >= self.config.max_failures
        {
            self.log(None, username, AccessEvent::LockedOut, ctx)
                .await?;
            return Err(AuthError::TooManyAttempts);
        }

        let user = match store::find_user_by_username(&self.pool, username).await? {
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
        store::insert_session(&self.pool, &hash_token(&token), user.id, expires_at).await?;
        self.log(Some(user.id), username, AccessEvent::LoginSuccess, ctx)
            .await?;

        Ok(IssuedSession { token, expires_at })
    }

    /// Resolves a raw session token to an identity, refreshing its last-seen
    /// time. Expired sessions, and sessions whose user was disabled or removed,
    /// resolve to [`AuthError::SessionInvalid`].
    pub async fn verify_session(&self, token: &str) -> Result<AuthenticatedUser, AuthError> {
        let token_hash = hash_token(token);
        let now = Utc::now();

        let user_id = store::find_valid_session(&self.pool, &token_hash, now)
            .await?
            .ok_or(AuthError::SessionInvalid)?;
        let user = store::find_active_user_by_id(&self.pool, user_id)
            .await?
            .ok_or(AuthError::SessionInvalid)?;
        store::touch_session(&self.pool, &token_hash, now).await?;

        let grants = store::load_role_permissions(&self.pool, user.id)
            .await?
            .into_iter()
            .flatten();
        let permissions = PermissionSet::new(user.is_superuser, grants);

        Ok(AuthenticatedUser { user, permissions })
    }

    /// Invalidates a session. Unknown tokens are a no-op.
    pub async fn logout(&self, token: &str) -> Result<(), AuthError> {
        store::delete_session(&self.pool, &hash_token(token)).await
    }

    async fn log(
        &self,
        user_id: Option<uuid::Uuid>,
        username: &str,
        event: AccessEvent,
        ctx: &RequestContext,
    ) -> Result<(), AuthError> {
        store::insert_access_log(
            &self.pool,
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

    async fn seed_user(
        pool: &PgPool,
        username: &str,
        password: &str,
        superuser: bool,
    ) -> uuid::Uuid {
        let hash = password::hash_password(password).unwrap();
        store::create_user(
            pool,
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

    fn service(pool: PgPool) -> AuthService {
        AuthService::new(pool, AuthConfig::default())
    }

    /// Applies this module's migrations through the framework runner, the same
    /// path the application uses at startup. Also exercises the runner itself.
    async fn migrate(pool: &PgPool) {
        laterite_core::migrate::run(pool, &[crate::migrations()])
            .await
            .expect("migrations should apply");
    }

    #[sqlx::test(migrations = false)]
    async fn authenticate_issues_a_verifiable_session(pool: PgPool) {
        migrate(&pool).await;
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

    #[sqlx::test(migrations = false)]
    async fn full_name_falls_back_to_first_name_when_last_is_absent(pool: PgPool) {
        migrate(&pool).await;
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

    #[sqlx::test(migrations = false)]
    async fn wrong_password_is_rejected(pool: PgPool) {
        migrate(&pool).await;
        seed_user(&pool, "root", "hunter2", false).await;
        let svc = service(pool);

        let err = svc
            .authenticate("root", "wrong", &RequestContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[sqlx::test(migrations = false)]
    async fn unknown_user_is_rejected_without_distinction(pool: PgPool) {
        migrate(&pool).await;
        let svc = service(pool);
        let err = svc
            .authenticate("ghost", "whatever", &RequestContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[sqlx::test(migrations = false)]
    async fn lockout_trips_after_max_failures(pool: PgPool) {
        migrate(&pool).await;
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

    #[sqlx::test(migrations = false)]
    async fn permissions_come_from_assigned_roles(pool: PgPool) {
        migrate(&pool).await;
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

    #[sqlx::test(migrations = false)]
    async fn logout_invalidates_the_session(pool: PgPool) {
        migrate(&pool).await;
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

    #[sqlx::test(migrations = false)]
    async fn inactive_account_is_refused_after_correct_password(pool: PgPool) {
        migrate(&pool).await;
        let user_id = seed_user(&pool, "root", "pw", false).await;
        sqlx::query!(
            "update backend_users set is_active = false where id = $1",
            user_id
        )
        .execute(&pool)
        .await
        .unwrap();

        let svc = service(pool);
        let err = svc
            .authenticate("root", "pw", &RequestContext::default())
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InactiveAccount));
    }
}
