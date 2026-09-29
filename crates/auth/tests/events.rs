//! The auth service announces what it records, once it is recorded.

use std::sync::{Arc, Mutex};

use laterite_auth::events::{LockedOut, PasswordChanged, SignInFailed, SignedIn, SignedOut};
use laterite_auth::{password, store, AuthConfig, AuthError, AuthService, RequestContext};
use laterite_core::strata::async_trait;
use laterite_core::{Actor, Db, Event, EventCx, EventError, Events, Listener};

const PASSWORD: &str = "correct-horse-1";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[laterite_auth::migrations()]).await
}

fn from(address: &str) -> RequestContext {
    RequestContext {
        ip_address: Some(address.to_string()),
        user_agent: Some("acme-browser".to_string()),
    }
}

async fn seed(db: &Db, username: &str) -> i64 {
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
    .unwrap()
}

type Heard = Arc<Mutex<Vec<String>>>;

/// Writes every event down as its name and its payload.
struct Recorder(Heard);

impl Recorder {
    fn note<E: Event>(&self, event: &E) {
        let payload = serde_json::to_string(event).unwrap();
        self.0
            .lock()
            .unwrap()
            .push(format!("{} {payload}", E::NAME));
    }
}

macro_rules! records {
    ($($event:ty),*) => {$(
        #[async_trait]
        impl Listener<$event> for Recorder {
            async fn handle(&self, _cx: &EventCx<'_>, event: &$event) -> Result<(), EventError> {
                self.note(event);
                Ok(())
            }
        }
    )*};
}
records!(
    SignedIn,
    SignInFailed,
    LockedOut,
    SignedOut,
    PasswordChanged
);

/// A service whose every announcement lands in the returned list.
fn listening(db: &Db) -> (AuthService, Heard) {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let events = Events::builder(db.clone())
        .listen::<SignedIn>(Recorder(heard.clone()))
        .listen::<SignInFailed>(Recorder(heard.clone()))
        .listen::<LockedOut>(Recorder(heard.clone()))
        .listen::<SignedOut>(Recorder(heard.clone()))
        .listen::<PasswordChanged>(Recorder(heard.clone()))
        .build()
        .unwrap();
    let service = AuthService::new(db.clone(), AuthConfig::default()).with_events(events);
    (service, heard)
}

fn taken(heard: &Heard) -> Vec<String> {
    std::mem::take(&mut *heard.lock().unwrap())
}

#[tokio::test]
async fn signing_in_and_out_is_announced() {
    let (db, _guard) = test_db().await;
    let id = seed(&db, "ada").await;
    let (svc, heard) = listening(&db);

    let session = svc
        .authenticate("ada", PASSWORD, &from("198.51.100.7"))
        .await
        .unwrap();
    assert_eq!(
        taken(&heard),
        [format!(
            r#"auth.signed_in {{"user_id":{id},"username":"ada","ip_address":"198.51.100.7","user_agent":"acme-browser"}}"#
        )]
    );

    svc.logout(&session.token).await.unwrap();
    assert_eq!(
        taken(&heard),
        [format!(r#"auth.signed_out {{"user_id":{id}}}"#)]
    );

    // The session is gone, so a second sign-out names nobody.
    svc.logout(&session.token).await.unwrap();
    assert!(taken(&heard).is_empty());
}

#[tokio::test]
async fn a_refused_credential_and_a_lockout_are_announced() {
    let (db, _guard) = test_db().await;
    let id = seed(&db, "ada").await;
    let (svc, heard) = listening(&db);
    let stranger = from("203.0.113.5");

    assert!(matches!(
        svc.authenticate("nobody", "wrong-guess", &stranger).await,
        Err(AuthError::InvalidCredentials)
    ));
    assert_eq!(
        taken(&heard),
        [
            r#"auth.sign_in_failed {"user_id":null,"username":"nobody","ip_address":"203.0.113.5","user_agent":"acme-browser"}"#
        ]
    );

    for _ in 0..AuthConfig::default().max_failures {
        let _ = svc.authenticate("ada", "wrong-guess", &stranger).await;
    }
    let failures = taken(&heard);
    assert_eq!(failures.len() as i64, AuthConfig::default().max_failures);
    assert!(failures
        .iter()
        .all(|line| line.starts_with(&format!(r#"auth.sign_in_failed {{"user_id":{id},"#))));

    assert!(matches!(
        svc.authenticate("ada", PASSWORD, &stranger).await,
        Err(AuthError::TooManyAttempts)
    ));
    assert_eq!(
        taken(&heard),
        [
            r#"auth.locked_out {"user_id":null,"username":"ada","ip_address":"203.0.113.5","user_agent":"acme-browser"}"#
        ]
    );
}

#[tokio::test]
async fn a_password_change_names_who_made_it() {
    let (db, _guard) = test_db().await;
    let id = seed(&db, "ada").await;
    let admin = seed(&db, "grace").await;
    let (svc, heard) = listening(&db);
    let home = from("198.51.100.7");
    let session = svc.authenticate("ada", PASSWORD, &home).await.unwrap();
    taken(&heard);

    svc.change_own_password(id, PASSWORD, "a-new-passphrase", &session.token, &home)
        .await
        .unwrap();
    assert_eq!(
        taken(&heard),
        [format!(
            r#"auth.password_changed {{"user_id":{id},"changed_by":{id}}}"#
        )]
    );

    svc.reset_operator_password(id, &Actor::user(admin, "grace"))
        .await
        .unwrap();
    assert_eq!(
        taken(&heard),
        [format!(
            r#"auth.password_changed {{"user_id":{id},"changed_by":{admin}}}"#
        )]
    );

    svc.reset_password("ada", "set-from-a-shell", &Actor::system("lat admin reset"))
        .await
        .unwrap();
    assert_eq!(
        taken(&heard),
        [format!(
            r#"auth.password_changed {{"user_id":{id},"changed_by":null}}"#
        )]
    );

    // A refused change announces nothing about the password.
    assert!(svc
        .change_password(id, "short", None, &Actor::system("test"))
        .await
        .is_err());
    assert!(taken(&heard).is_empty());
}

/// Fails on every sign-in.
struct Broken;

#[async_trait]
impl Listener<SignedIn> for Broken {
    async fn handle(&self, _cx: &EventCx<'_>, _event: &SignedIn) -> Result<(), EventError> {
        Err("the mail server is away".into())
    }
}

#[tokio::test]
async fn a_failing_listener_does_not_refuse_the_sign_in() {
    let (db, _guard) = test_db().await;
    seed(&db, "ada").await;
    let events = Events::builder(db.clone())
        .listen::<SignedIn>(Broken)
        .build()
        .unwrap();
    let svc = AuthService::new(db.clone(), AuthConfig::default()).with_events(events);

    let session = svc
        .authenticate("ada", PASSWORD, &from("198.51.100.7"))
        .await
        .unwrap();
    assert!(svc.verify_session(&session.token).await.is_ok());
}

#[tokio::test]
async fn a_payload_survives_a_round_trip() {
    let event = PasswordChanged::new(4, Some(4));
    assert!(event.by_owner());
    let back: PasswordChanged =
        serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
    assert_eq!(back, event);
    assert!(!PasswordChanged::new(4, Some(9)).by_owner());
    assert!(!PasswordChanged::new(4, None).by_owner());
}
