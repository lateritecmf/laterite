//! The access log has a screen of its own, gated by its permission. Imports
//! only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "correct-horse-1";

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

async fn signed_in(db: &Db, username: &str, superuser: bool) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    store::create_user(
        db,
        username,
        &format!("{username}@acme.test"),
        "Someone",
        None,
        &hash,
        superuser,
    )
    .await
    .unwrap();
    svc.authenticate(username, PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token
}

async fn get(db: &Db, path: &str, token: &str) -> (StatusCode, String) {
    let resp = app(db)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("laterite_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn the_screen_lists_sign_ins_newest_first() {
    let (db, _guard) = test_db().await;
    let token = signed_in(&db, "root", true).await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let _ = svc
        .authenticate("root", "wrong-guess", &RequestContext::default())
        .await;

    // The filter's own options name every event, so only the rows count.
    let rows = |html: &str| html.split("<tbody").nth(1).unwrap_or("").to_string();

    let (status, html) = get(&db, "/admin/access-log", &token).await;
    assert_eq!(status, StatusCode::OK);
    let body = rows(&html);
    // Stored codes read as their labels.
    let failure = body.find(">Failed<").expect("the failure is listed");
    let success = body.find(">Signed in<").expect("the sign-in is listed");
    assert!(failure < success, "newest first");
    assert!(
        !body.contains("login_success"),
        "the code itself is not shown: {body}"
    );

    let (status, html) = get(&db, "/admin/access-log?f_event=login_failure", &token).await;
    assert_eq!(status, StatusCode::OK);
    let body = rows(&html);
    assert!(body.contains(">Failed<"));
    assert!(!body.contains(">Signed in<"), "filtered: {body}");
}

#[tokio::test]
async fn the_screen_is_gated_by_its_permission() {
    let (db, _guard) = test_db().await;
    let token = signed_in(&db, "ada", false).await;
    let (status, _) = get(&db, "/admin/access-log", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
