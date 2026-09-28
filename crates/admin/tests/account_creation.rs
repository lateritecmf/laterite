//! Creating the first operator leaves an audit entry, credited to the process
//! since nobody is signed in yet. Imports only the public API.

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions::default(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

#[tokio::test]
async fn first_run_setup_is_audited() {
    let (db, _guard) = test_db().await;
    let body = "username=ada&first_name=Ada&last_name=&email=ada%40acme.test\
                &password=correct-horse-1&timezone=UTC";
    let resp = app(&db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/setup")
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_redirection(), "{}", resp.status());

    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let trail = svc.recent_audit(10).await.unwrap();
    let created = trail
        .iter()
        .find(|e| e.action == "backend.user.create")
        .expect("the creation is on the trail");
    assert_eq!(created.actor_username, "first-run setup");
    assert_eq!(created.actor_user_id, None);
    assert_eq!(created.target_type.as_deref(), Some("backend_user"));
    assert!(created.target_id.is_some());
    assert_eq!(created.target_label.as_deref(), Some("ada"));
}

/// The policy holds at the front door too: a short password makes no account.
#[tokio::test]
async fn setup_refuses_a_short_password() {
    let (db, _guard) = test_db().await;
    let body = "username=ada&first_name=Ada&last_name=&email=ada%40acme.test\
                &password=short&timezone=UTC";
    let resp = app(&db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/setup")
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("at least 8 characters"), "{html}");

    let svc = AuthService::new(db.clone(), AuthConfig::default());
    assert!(!svc.has_any_operator().await.unwrap());
}
