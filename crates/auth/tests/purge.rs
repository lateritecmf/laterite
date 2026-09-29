//! Expired sessions and credentials are swept; live ones stay.

use laterite_auth::{password, store, AuthConfig, AuthService, Purged, RequestContext};
use laterite_core::Db;
use std::time::Duration;

const PASSWORD: &str = "correct-horse-1";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&[laterite_auth::migrations()]).await
}

#[tokio::test]
async fn expired_rows_go_and_live_ones_stay() {
    let (db, _guard) = test_db().await;
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(&db, "ada", "ada@acme.test", "Ada", None, &hash, false)
        .await
        .unwrap();
    // Everything this service issues ends within a second.
    let mut short = AuthConfig::default();
    short.session_idle_timeout = Duration::from_secs(1);
    short.remember_duration = Duration::from_secs(1);
    let brief = AuthService::new(db.clone(), short);
    let long = AuthService::new(db.clone(), AuthConfig::default());
    let ctx = RequestContext::default();

    let old = brief
        .authenticate("ada", PASSWORD, &ctx)
        .await
        .unwrap()
        .token;
    brief.issue_remember(id).await.unwrap();
    // A revoked row past its expiry goes too.
    let revoked = brief
        .authenticate("ada", PASSWORD, &ctx)
        .await
        .unwrap()
        .token;
    long.sign_out_everywhere(id, &old).await.unwrap();
    let live = long
        .authenticate("ada", PASSWORD, &ctx)
        .await
        .unwrap()
        .token;

    assert_eq!(long.purge_expired().await.unwrap(), Purged::default());
    tokio::time::sleep(Duration::from_millis(1300)).await;

    let purged = long.purge_expired().await.unwrap();
    assert_eq!(purged.sessions, 2, "the old session and the revoked one");
    assert_eq!(purged.remember_tokens, 1);
    assert!(long.resolve_session(&live).await.is_ok());
    assert_eq!(
        long.session_end(&revoked).await.unwrap(),
        None,
        "gone, so nothing to tell"
    );
    assert_eq!(long.list_sessions(id, &live).await.unwrap().len(), 1);
    assert_eq!(long.purge_expired().await.unwrap(), Purged::default());
}
