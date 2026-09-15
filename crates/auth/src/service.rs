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
/// session_idle_timeout_secs = 7200
/// session_absolute_timeout_secs = 43200
/// remember_duration_secs = 1209600
/// max_failures = 5
/// failure_window_secs = 900
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// How long a session survives without a request. Activity past the halfway
    /// mark of this window pushes the deadline out again.
    #[serde(rename = "session_idle_timeout_secs", deserialize_with = "de_secs")]
    pub session_idle_timeout: Duration,
    /// The ceiling a session cannot be renewed past, counted from login. Bounds
    /// how long a stolen token stays useful however busy the thief keeps it.
    /// `session_ttl_secs` is the former name of this key and still reads.
    #[serde(
        rename = "session_absolute_timeout_secs",
        alias = "session_ttl_secs",
        deserialize_with = "de_secs"
    )]
    pub session_absolute_timeout: Duration,
    /// How long a "stay signed in" credential lasts. It survives the session
    /// ceiling: the point of it is to outlive the session and mint a new one.
    #[serde(rename = "remember_duration_secs", deserialize_with = "de_secs")]
    pub remember_duration: Duration,
    /// Failed attempts within `failure_window` before a username is locked out.
    pub max_failures: i64,
    /// The window over which failed attempts are counted.
    #[serde(rename = "failure_window_secs", deserialize_with = "de_secs")]
    pub failure_window: Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_idle_timeout: Duration::from_secs(60 * 60 * 2),
            session_absolute_timeout: Duration::from_secs(60 * 60 * 12),
            remember_duration: Duration::from_secs(60 * 60 * 24 * 14),
            max_failures: 5,
            failure_window: Duration::from_secs(60 * 15),
        }
    }
}

impl AuthConfig {
    /// When a session created at `created_at` and last seen at `last_seen`
    /// expires: the idle window measured from the last request, the ceiling
    /// measured from login, whichever falls first.
    fn deadline(&self, created_at: DateTime<Utc>, last_seen: DateTime<Utc>) -> DateTime<Utc> {
        let idle = last_seen + chrono_from_std(self.session_idle_timeout);
        let ceiling = created_at + chrono_from_std(self.session_absolute_timeout);
        idle.min(ceiling)
    }
}

/// Deserializes a whole-second count into a `Duration`.
fn de_secs<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
    Ok(Duration::from_secs(u64::deserialize(de)?))
}

#[cfg(test)]
mod auth_config_tests {
    use super::AuthConfig;
    use chrono::Utc;
    use std::time::Duration;

    #[test]
    fn unset_keys_keep_defaults() {
        let cfg: AuthConfig = serde_json::from_str(r#"{"max_failures": 3}"#).unwrap();
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.session_idle_timeout, Duration::from_secs(60 * 60 * 2));
        assert_eq!(
            cfg.session_absolute_timeout,
            Duration::from_secs(60 * 60 * 12)
        );
        assert_eq!(cfg.failure_window, Duration::from_secs(60 * 15));
    }

    #[test]
    fn seconds_map_to_durations() {
        let cfg: AuthConfig = serde_json::from_str(
            r#"{"session_idle_timeout_secs": 1800, "session_absolute_timeout_secs": 3600,
                 "max_failures": 7, "failure_window_secs": 120}"#,
        )
        .unwrap();
        assert_eq!(cfg.session_idle_timeout, Duration::from_secs(1800));
        assert_eq!(cfg.session_absolute_timeout, Duration::from_secs(3600));
        assert_eq!(cfg.max_failures, 7);
        assert_eq!(cfg.failure_window, Duration::from_secs(120));
    }

    #[test]
    fn the_idle_window_ends_a_quiet_session_first() {
        let cfg = AuthConfig::default();
        let login = Utc::now();
        // Seen just now, eleven hours into a twelve-hour ceiling: two hours idle
        // still falls first.
        let seen = login + chrono::Duration::hours(1);
        assert_eq!(cfg.deadline(login, seen), seen + chrono::Duration::hours(2));
    }

    #[test]
    fn the_ceiling_caps_a_session_kept_busy() {
        let cfg = AuthConfig::default();
        let login = Utc::now();
        // Active at the eleventh hour: the idle window would reach thirteen
        // hours, so the twelve-hour ceiling has to win.
        let seen = login + chrono::Duration::hours(11);
        assert_eq!(
            cfg.deadline(login, seen),
            login + chrono::Duration::hours(12)
        );
    }

    #[test]
    fn the_former_ttl_key_still_sets_the_ceiling() {
        let cfg: AuthConfig = serde_json::from_str(r#"{"session_ttl_secs": 3600}"#).unwrap();
        assert_eq!(cfg.session_absolute_timeout, Duration::from_secs(3600));
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
    /// Whose session it is, so the caller can issue a remember credential
    /// without looking the user up again.
    pub user_id: i64,
}

/// A freshly minted "stay signed in" credential. `cookie` is the raw
/// `selector:verifier` value for the client; only the verifier's hash is kept.
#[derive(Debug, Clone)]
pub struct RememberCredential {
    pub cookie: String,
    pub expires_at: DateTime<Utc>,
}

/// What a presented remember cookie produced: a new session, and the
/// replacement credential that must overwrite the cookie just used.
#[derive(Debug, Clone)]
pub struct RecalledSession {
    pub session: IssuedSession,
    pub remember: RememberCredential,
}

/// One of an account's live sessions, as the account's own sessions list shows
/// it. `id` is derived from the stored key and is safe to put in a page: it
/// names a row without being usable to authenticate as it.
#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Whether this is the session doing the asking.
    pub current: bool,
}

/// An authenticated backend user together with the permissions in force.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user: BackendUser,
    pub permissions: PermissionSet,
}

/// The acting operator as a record-layer actor, so a write attributes itself
/// without the admin mapping it by hand.
impl From<&AuthenticatedUser> for laterite_core::Actor {
    fn from(user: &AuthenticatedUser) -> Self {
        laterite_core::Actor::user(user.user.id, &user.user.username)
    }
}

/// A resolved live session: the authenticated identity plus the opaque data
/// blob the surface stored on it (`None` until the surface writes one). Auth
/// does not interpret the blob.
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub identity: AuthenticatedUser,
    pub data: Option<String>,
    /// When the session now expires, whichever clock runs out first. The login
    /// cookie is bounded by the ceiling, so this never outruns it.
    pub expires_at: DateTime<Utc>,
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

/// One append-only audit entry: who did what, to which target. Actions are
/// dot-keyed (`backend.role.update`); the username is snapshotted so the entry
/// stays legible after the actor's account is removed.
pub struct AuditEntry<'a> {
    /// The acting operator's id, or `None` for a system action.
    pub actor_user_id: Option<i64>,
    /// The acting operator's username, recorded verbatim.
    pub actor_username: &'a str,
    /// A dot-keyed action, e.g. `backend.plugin.disable`.
    pub action: &'a str,
    /// What was acted on (e.g. `backend_role`) and its id; both optional.
    pub target_type: Option<&'a str>,
    pub target_id: Option<&'a str>,
    /// Optional JSON describing the change.
    pub detail: Option<&'a str>,
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

    /// How long a session stays valid. A caller that persists the session in a
    /// cookie matches its lifetime to this, so the two cannot disagree.
    pub fn session_absolute_timeout(&self) -> Duration {
        self.config.session_absolute_timeout
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
        let expires_at = self.config.deadline(now, now);
        store::insert_session(&self.db, &hash_token(&token), user.id, expires_at).await?;
        self.log(Some(user.id), username, AccessEvent::LoginSuccess, ctx)
            .await?;

        Ok(IssuedSession {
            token,
            expires_at,
            user_id: user.id,
        })
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
        // Renewing on every request would cost a write per request for a
        // deadline that moves by seconds. Past the halfway mark of the idle
        // window the write buys real time, so that is where it happens.
        let idle = chrono_from_std(self.config.session_idle_timeout);
        let expires_at = self.config.deadline(session.created_at, now);
        if now - session.last_seen_at > idle / 2 {
            store::renew_session(&self.db, &token_hash, now, expires_at).await?;
        }

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
            expires_at,
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

    /// Issues a "stay signed in" credential for `user_id`, one row per device.
    pub async fn issue_remember(&self, user_id: i64) -> Result<RememberCredential, AuthError> {
        let (selector, verifier) = (generate_token(), generate_token());
        let expires_at = Utc::now() + chrono_from_std(self.config.remember_duration);
        store::insert_remember_token(
            &self.db,
            &selector,
            &hash_token(&verifier),
            user_id,
            expires_at,
        )
        .await?;
        Ok(RememberCredential {
            cookie: format!("{selector}:{verifier}"),
            expires_at,
        })
    }

    /// Trades a presented remember cookie for a fresh session and a replacement
    /// credential, rotating the stored row so a cookie is single-use.
    ///
    /// A selector that resolves with the wrong verifier means a copy of the
    /// cookie is in circulation: one of the two holders used it first and
    /// rotated it, and this is the other. Every credential the user holds is
    /// dropped, which signs out the thief at the cost of signing out the owner.
    pub async fn consume_remember(
        &self,
        cookie: &str,
        ctx: &RequestContext,
    ) -> Result<RecalledSession, AuthError> {
        let (selector, verifier) = cookie.split_once(':').ok_or(AuthError::SessionInvalid)?;
        let now = Utc::now();
        let found = store::find_remember_token(&self.db, selector, now)
            .await?
            .ok_or(AuthError::SessionInvalid)?;

        if !constant_time_eq(&hash_token(verifier), &found.verifier_hash) {
            tracing::warn!(
                user_id = found.user_id,
                "a stay-signed-in cookie was presented with a stale secret;                  dropping every credential for this account"
            );
            store::delete_user_remember_tokens(&self.db, found.user_id).await?;
            self.log(Some(found.user_id), "", AccessEvent::LoginFailure, ctx)
                .await?;
            return Err(AuthError::SessionInvalid);
        }

        // A disabled or removed account must not be recalled back in.
        let user = store::find_active_user_by_id(&self.db, found.user_id)
            .await?
            .ok_or(AuthError::SessionInvalid)?;

        // Rotate in place. The selector stays, so a copy of the spent cookie
        // comes back as a mismatch on a known series rather than as a stranger,
        // which is the only thing that makes theft visible at all.
        let replacement = generate_token();
        let remember_expires = now + chrono_from_std(self.config.remember_duration);
        store::rotate_remember_token(
            &self.db,
            selector,
            &hash_token(&replacement),
            remember_expires,
        )
        .await?;
        let remember = RememberCredential {
            cookie: format!("{selector}:{replacement}"),
            expires_at: remember_expires,
        };

        let token = generate_token();
        let expires_at = self.config.deadline(now, now);
        store::insert_session(&self.db, &hash_token(&token), user.id, expires_at).await?;
        self.log(
            Some(user.id),
            &user.username,
            AccessEvent::LoginSuccess,
            ctx,
        )
        .await?;

        Ok(RecalledSession {
            session: IssuedSession {
                token,
                expires_at,
                user_id: user.id,
            },
            remember,
        })
    }

    /// Drops the credential a cookie names. Unknown or malformed values are a
    /// no-op, so a stale cookie on logout is not an error.
    pub async fn revoke_remember(&self, cookie: &str) -> Result<(), AuthError> {
        if let Some((selector, _)) = cookie.split_once(':') {
            store::delete_remember_token(&self.db, selector).await?;
        }
        Ok(())
    }

    /// Lists the account's live sessions, newest activity first, marking the one
    /// `current_token` belongs to.
    pub async fn list_sessions(
        &self,
        user_id: i64,
        current_token: &str,
    ) -> Result<Vec<ActiveSession>, AuthError> {
        let current_hash = hash_token(current_token);
        let rows = store::list_user_sessions(&self.db, user_id, Utc::now()).await?;
        Ok(rows
            .into_iter()
            .map(|s| ActiveSession {
                id: public_session_id(&s.token_hash),
                created_at: s.created_at,
                last_seen_at: s.last_seen_at,
                expires_at: s.expires_at,
                current: s.token_hash == current_hash,
            })
            .collect())
    }

    /// Ends one of the account's sessions by its public id. Returns whether a
    /// session matched. Scoped to `user_id`, so the worst a forged id can do is
    /// end one of the caller's own sessions.
    pub async fn revoke_session(&self, user_id: i64, id: &str) -> Result<bool, AuthError> {
        let rows = store::list_user_sessions(&self.db, user_id, Utc::now()).await?;
        let Some(target) = rows
            .into_iter()
            .find(|s| constant_time_eq(&public_session_id(&s.token_hash), id))
        else {
            return Ok(false);
        };
        let done = store::delete_user_session(&self.db, user_id, &target.token_hash).await?;
        Ok(done > 0)
    }

    /// Ends every session but the caller's, and drops every stay-signed-in
    /// credential the account holds. The credentials have to go too: leaving
    /// them would let any signed-out device mint itself a new session on its
    /// next request, which is the opposite of what this asks for.
    pub async fn sign_out_everywhere(
        &self,
        user_id: i64,
        keep_token: &str,
    ) -> Result<u64, AuthError> {
        let ended =
            store::delete_user_sessions_except(&self.db, user_id, &hash_token(keep_token)).await?;
        store::delete_user_remember_tokens(&self.db, user_id).await?;
        Ok(ended)
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

    /// Appends an entry to the append-only audit log. Mutating admin paths call
    /// this after a successful change, so actions that affect privileges or data
    /// are recorded. Self-service preference changes are not audited.
    pub async fn record_audit(&self, entry: AuditEntry<'_>) -> Result<(), AuthError> {
        store::insert_audit_log(
            &self.db,
            entry.actor_user_id,
            entry.actor_username,
            entry.action,
            entry.target_type,
            entry.target_id,
            entry.detail,
        )
        .await
    }

    /// The most recent audit entries, newest first, for the admin audit view.
    pub async fn recent_audit(&self, limit: u64) -> Result<Vec<store::AuditRecord>, AuthError> {
        store::recent_audit(&self.db, limit).await
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

/// A page-safe name for a session row. Hashing the stored key again means the
/// id identifies a session without being the key: even if a rendered page leaks,
/// nothing in it can be presented as a session or looked up as one.
fn public_session_id(token_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"laterite:session-id:");
    hasher.update(token_hash.as_bytes());
    to_hex(&hasher.finalize())
}

/// Compares two equal-length hex digests without an early return.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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

    /// Backdates a session's idle clock, standing in for time passing.
    async fn set_last_seen(db: &Db, token: &str, seen: DateTime<Utc>) {
        store::renew_session(
            db,
            &hash_token(token),
            seen,
            seen + chrono::Duration::hours(2),
        )
        .await
        .unwrap();
    }

    async fn last_seen(db: &Db, token: &str) -> DateTime<Utc> {
        store::find_valid_session(db, &hash_token(token), Utc::now())
            .await
            .unwrap()
            .expect("session still valid")
            .last_seen_at
    }

    #[tokio::test]
    async fn a_request_past_the_halfway_mark_renews_the_session() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool.clone());

        let issued = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        // Ninety minutes of quiet, past the one-hour halfway mark of the
        // two-hour idle window.
        let stale = Utc::now() - chrono::Duration::minutes(90);
        set_last_seen(&pool, &issued.token, stale).await;

        let resolved = svc.resolve_session(&issued.token).await.unwrap();

        assert!(
            last_seen(&pool, &issued.token).await > stale,
            "a request past the halfway mark should push the idle clock"
        );
        assert!(
            resolved.expires_at > Utc::now() + chrono::Duration::minutes(115),
            "renewal should restore very nearly the full idle window"
        );
    }

    #[tokio::test]
    async fn verify_session_renews_like_the_path_it_wraps() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool.clone());

        let issued = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        let stale = Utc::now() - chrono::Duration::minutes(90);
        set_last_seen(&pool, &issued.token, stale).await;

        svc.verify_session(&issued.token).await.unwrap();

        // The thin wrapper must not become a way to read a session without
        // ageing it; that is the whole reason the store's session functions
        // are crate-visible.
        assert!(last_seen(&pool, &issued.token).await > stale);
    }

    #[tokio::test]
    async fn a_request_before_the_halfway_mark_writes_nothing() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool.clone());

        let issued = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        // Ten minutes of quiet: nowhere near the halfway mark, so the write
        // would buy minutes and cost a round-trip on every request.
        let recent = Utc::now() - chrono::Duration::minutes(10);
        set_last_seen(&pool, &issued.token, recent).await;

        svc.resolve_session(&issued.token).await.unwrap();

        assert_eq!(
            last_seen(&pool, &issued.token).await.timestamp(),
            recent.timestamp(),
            "a request inside the halfway mark should leave the row alone"
        );
    }

    #[tokio::test]
    async fn a_session_past_its_idle_window_no_longer_resolves() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool.clone());

        let issued = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        // renew_session writes last-seen and the deadline together, so a
        // backdated pair is exactly what an abandoned session looks like.
        let long_ago = Utc::now() - chrono::Duration::hours(3);
        set_last_seen(&pool, &issued.token, long_ago).await;

        assert!(matches!(
            svc.resolve_session(&issued.token).await,
            Err(AuthError::SessionInvalid)
        ));
    }

    #[tokio::test]
    async fn a_remember_cookie_is_single_use_and_rotates() {
        let (pool, _guard) = test_db().await;
        let uid = seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        let first = svc.issue_remember(uid).await.unwrap();
        let recalled = svc
            .consume_remember(&first.cookie, &RequestContext::default())
            .await
            .unwrap();

        assert_ne!(
            recalled.remember.cookie, first.cookie,
            "using a credential must replace its secret"
        );
        assert_eq!(
            recalled.remember.cookie.split_once(':').unwrap().0,
            first.cookie.split_once(':').unwrap().0,
            "the selector is the series and must survive rotation"
        );
        // The session it minted is real.
        svc.verify_session(&recalled.session.token).await.unwrap();
        // And the spent cookie is dead, so a copy taken from a stolen laptop
        // buys nothing once the owner's browser has used it.
        assert!(matches!(
            svc.consume_remember(&first.cookie, &RequestContext::default())
                .await,
            Err(AuthError::SessionInvalid)
        ));
    }

    #[tokio::test]
    async fn a_stale_secret_drops_every_credential_the_user_holds() {
        let (pool, _guard) = test_db().await;
        let uid = seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        let stolen = svc.issue_remember(uid).await.unwrap();
        let other_device = svc.issue_remember(uid).await.unwrap();
        // The owner's browser uses it first, rotating the row.
        let owner = svc
            .consume_remember(&stolen.cookie, &RequestContext::default())
            .await
            .unwrap();

        // The thief presents the copy they took. Same selector, stale secret:
        // proof a cookie is in two places.
        let selector = stolen.cookie.split_once(':').unwrap().0;
        let forged = format!("{selector}:{}", generate_token());
        assert!(matches!(
            svc.consume_remember(&forged, &RequestContext::default())
                .await,
            Err(AuthError::SessionInvalid)
        ));

        // Everything is revoked, the owner's fresh credential included. Signing
        // the owner out is the price of signing the thief out.
        for credential in [owner.remember.cookie, other_device.cookie] {
            assert!(matches!(
                svc.consume_remember(&credential, &RequestContext::default())
                    .await,
                Err(AuthError::SessionInvalid)
            ));
        }
    }

    #[tokio::test]
    async fn the_sessions_list_marks_the_one_asking() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        let first = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        let second = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();

        let listed = svc
            .list_sessions(first.user_id, &second.token)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed.iter().filter(|s| s.current).count(), 1);
        // The public id names a row without being the row's key.
        assert!(listed.iter().all(|s| !s.id.is_empty()));
    }

    #[tokio::test]
    async fn one_account_cannot_revoke_anothers_session() {
        let (pool, _guard) = test_db().await;
        seed_user(&pool, "root", "hunter2", true).await;
        seed_user(&pool, "mallory", "hunter2", false).await;
        let svc = service(pool);

        let victim = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        let attacker = svc
            .authenticate("mallory", "hunter2", &RequestContext::default())
            .await
            .unwrap();

        // The attacker has somehow learned the victim's public session id.
        let victims_id = svc
            .list_sessions(victim.user_id, &victim.token)
            .await
            .unwrap()
            .remove(0)
            .id;

        assert!(
            !svc.revoke_session(attacker.user_id, &victims_id)
                .await
                .unwrap(),
            "a revoke is scoped to its own account"
        );
        // And the victim is still signed in.
        svc.verify_session(&victim.token).await.unwrap();
    }

    #[tokio::test]
    async fn signing_out_everywhere_keeps_this_one_and_drops_credentials() {
        let (pool, _guard) = test_db().await;
        let uid = seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        let keep = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        let other = svc
            .authenticate("root", "hunter2", &RequestContext::default())
            .await
            .unwrap();
        let credential = svc.issue_remember(uid).await.unwrap();

        let ended = svc.sign_out_everywhere(uid, &keep.token).await.unwrap();

        assert_eq!(ended, 1);
        svc.verify_session(&keep.token).await.unwrap();
        assert!(matches!(
            svc.verify_session(&other.token).await,
            Err(AuthError::SessionInvalid)
        ));
        // Leaving the credential would let the signed-out device mint a new
        // session on its very next request.
        assert!(matches!(
            svc.consume_remember(&credential.cookie, &RequestContext::default())
                .await,
            Err(AuthError::SessionInvalid)
        ));
    }

    #[tokio::test]
    async fn revoking_a_credential_ends_it() {
        let (pool, _guard) = test_db().await;
        let uid = seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        let credential = svc.issue_remember(uid).await.unwrap();
        svc.revoke_remember(&credential.cookie).await.unwrap();

        assert!(matches!(
            svc.consume_remember(&credential.cookie, &RequestContext::default())
                .await,
            Err(AuthError::SessionInvalid)
        ));
    }

    #[tokio::test]
    async fn audit_entries_record_and_read_back_newest_first() {
        let (pool, _guard) = test_db().await;
        let actor = seed_user(&pool, "root", "hunter2", true).await;
        let svc = service(pool);

        svc.record_audit(AuditEntry {
            actor_user_id: Some(actor),
            actor_username: "root",
            action: "backend.role.update",
            target_type: Some("backend_role"),
            target_id: Some("42"),
            detail: Some(r#"{"added":["a.b"]}"#),
        })
        .await
        .unwrap();
        svc.record_audit(AuditEntry {
            actor_user_id: Some(actor),
            actor_username: "root",
            action: "backend.plugin.disable",
            target_type: Some("plugin"),
            target_id: Some("acme.widgets"),
            detail: None,
        })
        .await
        .unwrap();

        let entries = svc.recent_audit(10).await.unwrap();
        assert_eq!(entries.len(), 2);
        // Newest first.
        assert_eq!(entries[0].action, "backend.plugin.disable");
        assert_eq!(entries[0].target_id.as_deref(), Some("acme.widgets"));
        assert_eq!(entries[0].detail, None);
        assert_eq!(entries[1].action, "backend.role.update");
        assert_eq!(entries[1].actor_username, "root");
        assert_eq!(entries[1].actor_user_id, Some(actor));
        assert_eq!(entries[1].detail.as_deref(), Some(r#"{"added":["a.b"]}"#));
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
