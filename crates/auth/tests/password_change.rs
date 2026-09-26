//! Changing a password signs the account out everywhere else, and the devices
//! signed out are told why.

use laterite_auth::{
    password, store, AuthConfig, AuthService, RequestContext, RevokeReason, SessionEnd,
    MIN_PASSWORD_LENGTH,
};
use laterite_core::Db;

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

    svc.change_password(id, NEW, Some(&here)).await.unwrap();

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
    svc.change_password(id, NEW, None).await.unwrap();
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

    svc.change_password(id, NEW, None).await.unwrap();

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
    svc.change_password(id, NEW, None).await.unwrap();
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
    assert!(svc.reset_password("ada", NEW).await.unwrap());
    assert!(svc.resolve_session(&a).await.is_err());
    assert!(svc.resolve_session(&b).await.is_err());
    assert!(
        !svc.reset_password("nobody", NEW).await.unwrap(),
        "an unknown name is reported"
    );
}

#[tokio::test]
async fn a_short_password_is_refused_and_changes_nothing() {
    let (db, _guard) = test_db().await;
    let (svc, id) = setup(&db).await;
    let here = sign_in(&svc).await;
    let short = "x".repeat(MIN_PASSWORD_LENGTH - 1);
    assert!(svc.change_password(id, &short, None).await.is_err());
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
    svc.change_password(id, NEW, None).await.unwrap();

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
    svc.change_password(id, NEW, None).await.unwrap();

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
