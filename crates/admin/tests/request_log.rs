//! The request log: an unserved URL is counted, and the log is a screen of
//! its own, gated by its permission. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions::default(),
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// A superuser, and an operator holding nothing.
async fn sessions(db: &Db) -> (String, String) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Root",
        last_name: None,
        password: "rootpw12345",
        timezone: None,
    })
    .await
    .unwrap();
    let hash = password::hash_password("plainpw12345").unwrap();
    store::create_user(db, "plain", "plain@acme.test", "Plain", None, &hash, false)
        .await
        .unwrap();
    let mut tokens = Vec::new();
    for (user, pw) in [("root", "rootpw12345"), ("plain", "plainpw12345")] {
        tokens.push(
            svc.authenticate(user, pw, &RequestContext::default())
                .await
                .unwrap()
                .token,
        );
    }
    (tokens.remove(0), tokens.remove(0))
}

async fn get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn an_unserved_url_is_counted_and_the_log_is_gated() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let (root, plain) = sessions(&db).await;

    for _ in 0..2 {
        let (status, _) = get(&app, "/admin/no-such-page", &root).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = get(&app, "/admin/another", &root).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, html) = get(&app, "/admin/request-log", &root).await;
    assert_eq!(status, StatusCode::OK);
    let row = html.find("/admin/no-such-page").expect(&html);
    let after = &html[row..];
    assert!(after.contains(">2<"), "counted twice: {after}");
    assert!(html.contains("/admin/another"), "{html}");
    assert!(html.contains(">GET<") && html.contains(">404<"), "{html}");

    // Clearing a row returns to the log, not to a doubled path.
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.set_session_data(&root, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/admin/request-log/delete")
        .header("cookie", format!("{SESSION_COOKIE}={root}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from("id=1"))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/admin/request-log")
    );
    let (_, html) = get(&app, "/admin/request-log", &root).await;
    assert!(
        !html.contains("/admin/no-such-page") && html.contains("Deleted 1 record."),
        "{html}"
    );

    let (status, _) = get(&app, "/admin/request-log", &plain).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "gated by its permission");
    let (_, html) = get(&app, "/admin/settings", &plain).await;
    assert!(
        !html.contains("Request Log"),
        "and out of their settings: {html}"
    );
}
