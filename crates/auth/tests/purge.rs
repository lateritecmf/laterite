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

    // Past the second those were issued for, however long hashing took to get
    // here; the live session is issued only now, so no clock decides the count.
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let live = long
        .authenticate("ada", PASSWORD, &ctx)
        .await
        .unwrap()
        .token;

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

#[tokio::test]
async fn a_sign_out_is_on_the_access_log_and_the_log_is_kept_to_its_retention() {
    let (db, _guard) = test_db().await;
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(&db, "ada", "ada@acme.test", "Ada", None, &hash, false)
        .await
        .unwrap();
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let ctx = RequestContext {
        ip_address: Some("203.0.113.9".to_string()),
        user_agent: Some("acme-browser".to_string()),
    };
    let token = svc.authenticate("ada", PASSWORD, &ctx).await.unwrap().token;
    svc.logout_with(&token, &ctx).await.unwrap();
    let log = store::recent_access_log(&db, id, 10).await.unwrap();
    assert_eq!(log[0].event, "logout");
    assert_eq!(log[0].ip_address.as_deref(), Some("203.0.113.9"));
    assert_eq!(log[1].event, "login_success");

    // Nothing is a year old yet, so the default retention removes nothing, and
    // a retention of zero never does.
    assert_eq!(svc.purge_expired().await.unwrap().access_log, 0);
    let mut keep_all = AuthConfig::default();
    keep_all.access_log_retention_days = 0;
    let forever = AuthService::new(db.clone(), keep_all);
    assert_eq!(forever.purge_expired().await.unwrap().access_log, 0);
    assert_eq!(
        store::recent_access_log(&db, id, 10).await.unwrap().len(),
        2
    );
}
