//! Changing a password signs the account out everywhere else, and the devices
//! signed out are told why.

use laterite_auth::{
    password, store, AuthConfig, AuthService, NewOperator, RequestContext, RevokeReason,
    SessionEnd, MIN_PASSWORD_LENGTH,
};
use laterite_core::{Actor, Db};

const OLD: &str = "correct-horse-1";
const NEW: &str = "battery-staple-2";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[laterite_auth::migrations()]).await
}

async fn setup(db: &Db) -> (AuthService, i64) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(OLD).unwrap();
    let id = store::create_user(db, "ada", "ada@acme.test", "Ada", None, &hash, false)
        .await
        .unwrap();
    (svc, id)
}

fn system() -> Actor {
    Actor::system("lat admin reset-password")
}

async fn sign_in(svc: &AuthService) -> String {
    svc.authenticate("ada", OLD, &RequestContext::default())
        .await
        .unwrap()
        .token
}

#[tokio::test]
async fn every_other_session_is_ended_and_told_why() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let here = sign_in(&svc).await;
    let laptop = sign_in(&svc).await;

    svc.change_password(id, NEW, Some(&here), &Actor::user(id, "ada"))
        .await
        .unwrap();

    assert!(
        svc.resolve_session(&here).await.is_ok(),
        "the session it was changed from survives"
    );
    assert!(
        svc.resolve_session(&laptop).await.is_err(),
        "the other one no longer works"
    );
    assert_eq!(
        svc.session_end(&laptop).await.unwrap(),
        Some(SessionEnd::Revoked(RevokeReason::PasswordChanged)),
        "and knows why"
    );
}

/// The reason is said once: a stale tab reloaded later is simply signed out.
#[tokio::test]
async fn the_reason_is_given_once() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let laptop = sign_in(&svc).await;
    svc.change_password(id, NEW, None, &system()).await.unwrap();
    assert!(svc.session_end(&laptop).await.unwrap().is_some());
    assert_eq!(svc.session_end(&laptop).await.unwrap(), None);
}

/// A stay-signed-in cookie would otherwise mint a fresh session on its next
/// request, undoing the change for whoever else holds the account.
#[tokio::test]
async fn stay_signed_in_credentials_stop_working() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let remembered = svc.issue_remember(id).await.unwrap();

    svc.change_password(id, NEW, None, &system()).await.unwrap();

    assert!(
        svc.consume_remember(&remembered.cookie, &RequestContext::default())
            .await
            .is_err(),
        "the cookie mints nothing"
    );
}

#[tokio::test]
async fn the_new_password_works_and_the_old_one_does_not() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    svc.change_password(id, NEW, None, &system()).await.unwrap();
    assert!(svc
        .authenticate("ada", NEW, &RequestContext::default())
        .await
        .is_ok());
    assert!(svc
        .authenticate("ada", OLD, &RequestContext::default())
        .await
        .is_err());
}

/// The command-line reset runs from outside every session, so it keeps none.
#[tokio::test]
async fn a_reset_by_username_signs_out_every_session() {
    let (db, _guard) = test_db().await;
    let (svc, _) = setup(&db).await;
    let a = sign_in(&svc).await;
    let b = sign_in(&svc).await;
    assert!(svc.reset_password("ada", NEW, &system()).await.unwrap());
    assert!(svc.resolve_session(&a).await.is_err());
    assert!(svc.resolve_session(&b).await.is_err());
    assert!(
        !svc.reset_password("nobody", NEW, &system()).await.unwrap(),
        "an unknown name is reported"
    );
}

#[tokio::test]
async fn a_short_password_is_refused_and_changes_nothing() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let here = sign_in(&svc).await;
    let short = "x".repeat(MIN_PASSWORD_LENGTH - 1);
    assert!(svc
        .change_password(id, &short, None, &system())
        .await
        .is_err());
    assert!(
        svc.resolve_session(&here).await.is_ok(),
        "nobody was signed out"
    );
    assert!(svc
        .authenticate("ada", OLD, &RequestContext::default())
        .await
        .is_ok());
}

#[tokio::test]
async fn deactivation_and_signing_out_elsewhere_carry_their_own_reasons() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let here = sign_in(&svc).await;
    let there = sign_in(&svc).await;
    svc.sign_out_everywhere(id, &here).await.unwrap();
    assert_eq!(
        svc.session_end(&there).await.unwrap(),
        Some(SessionEnd::Revoked(RevokeReason::SignedOutElsewhere))
    );

    let admin = store::create_user(
        &db,
        "root",
        "root@acme.test",
        "Root",
        None,
        &password::hash_password("rootpw12345").unwrap(),
        true,
    )
    .await
    .unwrap();
    svc.set_user_active(admin, id, false).await.unwrap();
    assert_eq!(
        svc.session_end(&here).await.unwrap(),
        Some(SessionEnd::Revoked(RevokeReason::Deactivated))
    );
}

#[tokio::test]
async fn a_token_never_issued_has_no_end() {
    let (db, _guard) = test_db().await;
    let (svc, _) = setup(&db).await;
    assert_eq!(svc.session_end("never-issued").await.unwrap(), None);
}

/// A device returning on its stay-signed-in cookie alone is told why too, once,
/// and only if it holds the current secret.
#[tokio::test]
async fn a_remembered_device_is_told_why_once() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let remembered = svc.issue_remember(id).await.unwrap();
    svc.change_password(id, NEW, None, &system()).await.unwrap();

    let (selector, _) = remembered.cookie.split_once(':').unwrap();
    assert_eq!(
        svc.remember_end(&format!("{selector}:not-the-secret"))
            .await
            .unwrap(),
        None,
        "a copy without the current secret learns nothing"
    );
    assert_eq!(
        svc.remember_end(&remembered.cookie).await.unwrap(),
        Some(RevokeReason::PasswordChanged)
    );
    assert_eq!(svc.remember_end(&remembered.cookie).await.unwrap(), None);
}

/// Presenting a revoked cookie must fail quietly, not as a stolen copy: theft
/// detection drops every credential, which would erase the reason with it.
#[tokio::test]
async fn a_revoked_cookie_does_not_look_stolen() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let remembered = svc.issue_remember(id).await.unwrap();
    svc.change_password(id, NEW, None, &system()).await.unwrap();

    assert!(svc
        .consume_remember(&remembered.cookie, &RequestContext::default())
        .await
        .is_err());
    assert_eq!(
        svc.remember_end(&remembered.cookie).await.unwrap(),
        Some(RevokeReason::PasswordChanged),
        "the credential survived the attempt, so theft detection did not fire"
    );
}

/// The change is on the audit trail, attributed, with no trace of the password.
#[tokio::test]
async fn the_change_is_audited_and_timed() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let created = svc
        .password_changed_at(id)
        .await
        .unwrap()
        .expect("the password was set at creation");

    svc.change_password(id, NEW, None, &Actor::user(id, "ada"))
        .await
        .unwrap();
    svc.reset_password("ada", OLD, &system()).await.unwrap();

    let trail = svc.recent_audit(10).await.unwrap();
    let changes: Vec<_> = trail
        .iter()
        .filter(|e| e.action == "backend.user.password_change")
        .collect();
    assert_eq!(changes.len(), 2);
    let by = |name: &str| changes.iter().find(|e| e.actor_username == name).unwrap();
    assert_eq!(by("lat admin reset-password").actor_user_id, None);
    assert_eq!(by("ada").actor_user_id, Some(id));
    assert!(changes
        .iter()
        .all(|e| e.target_id.as_deref() == Some(id.to_string().as_str())));
    assert!(changes.iter().all(|e| e.detail.is_none()));
    assert!(svc.password_changed_at(id).await.unwrap() > Some(created));
}

/// A refused password is not a change.
#[tokio::test]
async fn a_refused_password_leaves_no_record() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let before = svc.password_changed_at(id).await.unwrap();
    assert!(svc
        .change_password(id, "short", None, &system())
        .await
        .is_err());
    assert!(svc.recent_audit(10).await.unwrap().is_empty());
    assert_eq!(svc.password_changed_at(id).await.unwrap(), before);
}

/// An id nobody holds changes nothing and leaves no audit entry: the whole
/// change is one transaction, or none of it.
#[tokio::test]
async fn an_unknown_account_leaves_no_trace() {
    let (db, _guard) = test_db().await;
    let (svc, _) = setup(&db).await;
    assert!(svc
        .change_password(9999, NEW, None, &system())
        .await
        .is_err());
    assert!(svc.recent_audit(10).await.unwrap().is_empty());
}

/// Signing one device out from Preferences tells it why, the same as signing
/// out every other device does.
#[tokio::test]
async fn signing_out_one_device_tells_it_why() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let here = sign_in(&svc).await;
    let laptop = sign_in(&svc).await;
    let other = svc
        .list_sessions(id, &here)
        .await
        .unwrap()
        .into_iter()
        .find(|s| !s.current)
        .unwrap();

    assert!(svc.revoke_session(id, &other.id).await.unwrap());

    assert!(svc.resolve_session(&here).await.is_ok());
    assert_eq!(
        svc.session_end(&laptop).await.unwrap(),
        Some(SessionEnd::Revoked(RevokeReason::SignedOutElsewhere))
    );
}

#[tokio::test]
async fn an_operator_changes_their_own_password_and_stays_signed_in() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let here = sign_in(&svc).await;
    let laptop = sign_in(&svc).await;
    let ctx = RequestContext::default();

    svc.change_own_password(id, OLD, NEW, &here, &ctx)
        .await
        .unwrap();

    assert!(svc.resolve_session(&here).await.is_ok());
    assert!(svc.resolve_session(&laptop).await.is_err());
    let trail = svc.recent_audit(10).await.unwrap();
    assert_eq!(trail[0].action, "backend.user.password_change");
    assert_eq!(trail[0].actor_user_id, Some(id));
}

/// Guessing the current password through a session counts toward the lockout.
#[tokio::test]
async fn a_wrong_current_password_changes_nothing_and_counts_as_a_failure() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let here = sign_in(&svc).await;
    let ctx = RequestContext::default();

    for _ in 0..AuthConfig::default().max_failures {
        assert!(matches!(
            svc.change_own_password(id, "wrong-guess", NEW, &here, &ctx)
                .await,
            Err(laterite_auth::AuthError::InvalidCredentials)
        ));
    }
    assert!(matches!(
        svc.change_own_password(id, OLD, NEW, &here, &ctx).await,
        Err(laterite_auth::AuthError::TooManyAttempts)
    ));
    assert!(
        svc.authenticate("ada", OLD, &RequestContext::default())
            .await
            .is_err(),
        "locked out, the old password still stands"
    );
}

/// One policy, read from the config, for every place a password is set.
#[tokio::test]
async fn the_policy_applies_wherever_a_password_is_set() {
    let (db, _guard) = test_db().await;
    let mut config = AuthConfig::default();
    config.password_policy.min_length = 12;
    let svc = AuthService::new(db.clone(), config);
    let operator = |password: &'static str| NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Root",
        last_name: None,
        password,
        timezone: None,
    };

    assert!(svc.create_superuser(operator("eleven-char")).await.is_err());
    assert!(
        !svc.has_any_operator().await.unwrap(),
        "nothing was created"
    );
    let id = svc
        .create_superuser(operator("twelve-chars"))
        .await
        .unwrap();

    assert!(svc
        .change_password(id, "eleven-char", None, &system())
        .await
        .is_err());
    assert!(svc
        .change_password(id, "another-dozen", None, &system())
        .await
        .is_ok());
}
