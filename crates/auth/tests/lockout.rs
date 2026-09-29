//! A lockout is per username and address, with a ceiling per address.

use laterite_auth::{password, store, AuthConfig, AuthError, AuthService, RequestContext};
use laterite_core::Db;

const PASSWORD: &str = "correct-horse-1";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[laterite_auth::migrations()]).await
}

fn from(address: &str) -> RequestContext {
    RequestContext {
        ip_address: Some(address.to_string()),
        user_agent: None,
    }
}

async fn seed(db: &Db, username: &str) {
    let hash = password::hash_password(PASSWORD).unwrap();
    store::create_user(
        db,
        username,
        &format!("{username}@acme.test"),
        "Someone",
        None,
        &hash,
        false,
    )
    .await
    .unwrap();
}

/// Five wrong guesses from one address lock that address out of the account;
/// the account's holder, elsewhere, signs in as before.
#[tokio::test]
async fn a_stranger_cannot_lock_an_operator_out_from_elsewhere() {
    let (db, _guard) = test_db().await;
    seed(&db, "ada").await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let stranger = from("203.0.113.5");
    let home = from("198.51.100.7");

    for _ in 0..AuthConfig::default().max_failures {
        assert!(matches!(
            svc.authenticate("ada", "wrong-guess", &stranger).await,
            Err(AuthError::InvalidCredentials)
        ));
    }
    assert!(matches!(
        svc.authenticate("ada", PASSWORD, &stranger).await,
        Err(AuthError::TooManyAttempts)
    ));
    assert!(svc.authenticate("ada", PASSWORD, &home).await.is_ok());
    assert!(svc.is_locked_out("ada").await.unwrap(), "from somewhere");

    svc.unlock("ada").await.unwrap();
    assert!(!svc.is_locked_out("ada").await.unwrap());
    assert!(svc.authenticate("ada", PASSWORD, &stranger).await.is_ok());
}

/// One address guessing across many usernames hits the address ceiling.
#[tokio::test]
async fn an_address_is_limited_across_usernames() {
    let (db, _guard) = test_db().await;
    seed(&db, "ada").await;
    let mut config = AuthConfig::default();
    config.max_failures_per_address = 4;
    let svc = AuthService::new(db.clone(), config);
    let stuffer = from("203.0.113.9");

    for name in ["alice", "bob", "carol", "dave"] {
        let _ = svc.authenticate(name, "wrong-guess", &stuffer).await;
    }
    assert!(matches!(
        svc.authenticate("ada", PASSWORD, &stuffer).await,
        Err(AuthError::TooManyAttempts)
    ));
    assert!(
        svc.authenticate("ada", PASSWORD, &from("198.51.100.7"))
            .await
            .is_ok(),
        "only that address is refused"
    );
}
