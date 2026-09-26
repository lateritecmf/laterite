//! A session ended on purpose sends its holder to the login screen with the
//! reason, and the login screen says it. Imports only the public API.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const COOKIE: &str = "laterite_session";
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

/// A signed-in superuser, and the service that manages them.
async fn signed_in(db: &Db) -> (AuthService, i64, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(db, "ada", "ada@acme.test", "Ada", None, &hash, true)
        .await
        .unwrap();
    let token = svc
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;
    (svc, id, token)
}

async fn get(db: &Db, path: &str, token: &str, htmx: bool) -> axum::response::Response {
    let mut req = Request::builder()
        .uri(path)
        .header("cookie", format!("{COOKIE}={token}"));
    if htmx {
        req = req.header("hx-request", "true");
    }
    app(db)
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn location(resp: &axum::response::Response) -> String {
    resp.headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

async fn body(resp: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_changed_password_sends_the_other_device_to_login_with_the_reason() {
    let (db, _guard) = test_db().await;
    let (svc, id, laptop) = signed_in(&db).await;
    svc.change_password(
        id,
        "battery-staple-2",
        None,
        &laterite_core::Actor::system("test"),
    )
    .await
    .unwrap();

    let resp = get(&db, "/admin", &laptop, false).await;
    assert!(resp.status().is_redirection());
    assert_eq!(location(&resp), "/admin/login?ended=password_changed");
}

#[tokio::test]
async fn the_login_screen_says_why() {
    let (db, _guard) = test_db().await;
    let _ = signed_in(&db).await;
    let resp = app(&db)
        .oneshot(
            Request::builder()
                .uri("/admin/login?ended=password_changed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body(resp).await;
    assert!(html.contains("Your password was changed"), "{html}");
    assert!(
        html.contains("data-lat-persist"),
        "shown as a toast that stays until dismissed"
    );
}

/// Nothing from the URL reaches the page: an unknown code shows nothing.
#[tokio::test]
async fn an_unknown_code_shows_no_message() {
    let (db, _guard) = test_db().await;
    let _ = signed_in(&db).await;
    let resp = app(&db)
        .oneshot(
            Request::builder()
                .uri("/admin/login?ended=%3Cscript%3Ealert(1)%3C/script%3E")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body(resp).await;
    assert!(!html.contains("lat-flash"));
    assert!(!html.contains("<script>alert(1)"));
}

/// An htmx swap must not render the login page inside the region that asked.
#[tokio::test]
async fn an_htmx_request_is_told_to_navigate_rather_than_swap() {
    let (db, _guard) = test_db().await;
    let (svc, id, laptop) = signed_in(&db).await;
    svc.change_password(
        id,
        "battery-staple-2",
        None,
        &laterite_core::Actor::system("test"),
    )
    .await
    .unwrap();

    let resp = get(&db, "/admin", &laptop, true).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        resp.headers().get("hx-redirect").unwrap(),
        "/admin/login?ended=password_changed"
    );
}

#[tokio::test]
async fn deactivation_is_named_too() {
    let (db, _guard) = test_db().await;
    let (svc, id, token) = signed_in(&db).await;
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
    let resp = get(&db, "/admin", &token, false).await;
    assert_eq!(location(&resp), "/admin/login?ended=deactivated");
}

/// A token nobody issued is simply signed out, with no reason invented.
#[tokio::test]
async fn an_unknown_session_goes_to_plain_login() {
    let (db, _guard) = test_db().await;
    let _ = signed_in(&db).await;
    let resp = get(&db, "/admin", "never-issued", false).await;
    assert_eq!(location(&resp), "/admin/login");
}

/// The reason is told once; a second visit from the same stale tab is plain.
#[tokio::test]
async fn the_reason_is_told_once() {
    let (db, _guard) = test_db().await;
    let (svc, id, laptop) = signed_in(&db).await;
    svc.change_password(
        id,
        "battery-staple-2",
        None,
        &laterite_core::Actor::system("test"),
    )
    .await
    .unwrap();
    let _ = get(&db, "/admin", &laptop, false).await;
    let again = get(&db, "/admin", &laptop, false).await;
    assert_eq!(location(&again), "/admin/login");
}

/// A device whose session is gone and which returns on its stay-signed-in
/// cookie alone is told why as well.
#[tokio::test]
async fn a_remembered_device_is_told_why() {
    let (db, _guard) = test_db().await;
    let (svc, id, _) = signed_in(&db).await;
    let remembered = svc.issue_remember(id).await.unwrap();
    svc.change_password(
        id,
        "battery-staple-2",
        None,
        &laterite_core::Actor::system("test"),
    )
    .await
    .unwrap();

    let resp = app(&db)
        .oneshot(
            Request::builder()
                .uri("/admin")
                .header("cookie", format!("laterite_remember={}", remembered.cookie))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(location(&resp), "/admin/login?ended=password_changed");
}
