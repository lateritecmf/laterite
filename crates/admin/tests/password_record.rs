//! When a password last changed is shown to the account holder and to the
//! administrators who manage it. Imports only the public API.

use axum::body::Body;
use axum::http::Request;
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, RequestContext};
use laterite_core::{Actor, CatalogStore, Db};
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

async fn page(db: &Db, path: &str, token: &str) -> String {
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
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn both_screens_show_when_the_password_was_set() {
    let (db, _guard) = test_db().await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(&db, "ada", "ada@acme.test", "Ada", None, &hash, true)
        .await
        .unwrap();
    let token = svc
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;

    // Creation sets the password, so a new account already has a date.
    let users = format!("/admin/users/{id}/edit");
    for html in [
        page(&db, &users, &token).await,
        page(&db, "/admin/preferences", &token).await,
    ] {
        assert!(!html.contains("Not recorded"), "{html}");
    }

    svc.change_password(
        id,
        "battery-staple-2",
        Some(&token),
        &Actor::user(id, "ada"),
    )
    .await
    .unwrap();

    for html in [
        page(&db, &users, &token).await,
        page(&db, "/admin/preferences", &token).await,
    ] {
        assert!(html.contains("Password"), "{html}");
        assert!(!html.contains("Not recorded"), "{html}");
    }
}

async fn post_password(
    db: &Db,
    token: &str,
    extra_cookie: &str,
    current: &str,
    new: &str,
    confirm: &str,
) -> axum::response::Response {
    // The session's CSRF token is on the Preferences page.
    let html = page(db, "/admin/preferences", token).await;
    let csrf = html
        .split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_string();
    let body = format!(
        "_csrf={csrf}&current_password={current}&new_password={new}&confirm_password={confirm}"
    );
    app(db)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/preferences/password")
                .header("cookie", format!("laterite_session={token}{extra_cookie}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn operator(db: &Db) -> (AuthService, i64, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password(PASSWORD).unwrap();
    let id = store::create_user(db, "ada", "ada@acme.test", "Ada", None, &hash, false)
        .await
        .unwrap();
    let token = svc
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;
    (svc, id, token)
}

#[tokio::test]
async fn the_preferences_form_changes_the_password_and_keeps_this_session() {
    let (db, _guard) = test_db().await;
    let (svc, _, here) = operator(&db).await;
    let laptop = svc
        .authenticate("ada", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;

    let resp = post_password(
        &db,
        &here,
        "",
        PASSWORD,
        "battery-staple-2",
        "battery-staple-2",
    )
    .await;
    assert!(resp.status().is_redirection());
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/admin/preferences"
    );
    assert!(svc.resolve_session(&here).await.is_ok());
    assert!(svc.resolve_session(&laptop).await.is_err());
    assert!(page(&db, "/admin/preferences", &here)
        .await
        .contains("Password changed."));
}

#[tokio::test]
async fn the_preferences_form_refuses_with_the_reason() {
    let (db, _guard) = test_db().await;
    let (svc, id, here) = operator(&db).await;
    let before = svc.password_changed_at(id).await.unwrap();
    for (current, new, confirm, says) in [
        (
            PASSWORD,
            "battery-staple-2",
            "battery-staple-3",
            "do not match",
        ),
        (PASSWORD, "short", "short", "at least 8 characters"),
        (
            "wrong-guess",
            "battery-staple-2",
            "battery-staple-2",
            "not correct",
        ),
    ] {
        let resp = post_password(&db, &here, "", current, new, confirm).await;
        assert_eq!(resp.status(), 200);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains(says), "{says}: {html}");
    }
    assert_eq!(svc.password_changed_at(id).await.unwrap(), before);
}

/// The browser the change was made from keeps its stay-signed-in credential.
#[tokio::test]
async fn this_browser_stays_signed_in() {
    let (db, _guard) = test_db().await;
    let (svc, id, here) = operator(&db).await;
    let old = svc.issue_remember(id).await.unwrap().cookie;

    let resp = post_password(
        &db,
        &here,
        &format!("; laterite_remember={old}"),
        PASSWORD,
        "battery-staple-2",
        "battery-staple-2",
    )
    .await;
    let reissued = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.strip_prefix("laterite_remember="))
        .and_then(|v| v.split(';').next())
        .unwrap()
        .replace("%3A", ":");
    assert_ne!(reissued, old);
    assert!(svc
        .consume_remember(&reissued, &RequestContext::default())
        .await
        .is_ok());
}
