//! An account holding a temporary password can reach nothing but the
//! Preferences form that replaces it. Imports only the public API.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
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

/// A signed-in superuser whose password was made for them.
async fn temporary(db: &Db) -> (AuthService, i64, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(db, "ada", "ada@acme.test", "Ada", None, &hash, true)
        .await
        .unwrap();
    svc.require_password_change(id).await.unwrap();
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
        .header("cookie", format!("laterite_session={token}"));
    if htmx {
        req = req.header("hx-request", "true");
    }
    app(db)
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn every_screen_sends_the_operator_to_the_password_form() {
    let (db, _guard) = test_db().await;
    let (_, _, token) = temporary(&db).await;

    let resp = get(&db, "/admin", &token, false).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/admin/preferences#password"
    );

    let resp = get(&db, "/admin/users", &token, true).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        resp.headers().get("hx-redirect").unwrap(),
        "/admin/preferences#password"
    );

    let resp = get(&db, "/admin/preferences", &token, false).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("This password is temporary"), "{html}");
}

#[tokio::test]
async fn setting_their_own_password_opens_the_panel_again() {
    let (db, _guard) = test_db().await;
    let (svc, id, token) = temporary(&db).await;

    let html = body(get(&db, "/admin/preferences", &token, false).await).await;
    let csrf = html
        .split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string();
    let form = format!(
        "_csrf={csrf}&current_password={PASSWORD}&new_password=battery-staple-2\
         &confirm_password=battery-staple-2"
    );
    let resp = app(&db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/preferences/password")
                .header("cookie", format!("laterite_session={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    assert!(!svc.must_change_password(id).await.unwrap());

    let resp = get(&db, "/admin", &token, false).await;
    assert_eq!(resp.status(), StatusCode::OK, "the panel opens again");
}
