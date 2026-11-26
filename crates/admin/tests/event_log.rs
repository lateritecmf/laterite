//! The event log screen: what the application logged, listed and opened to
//! read, with no way to write a row. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::eventlog::{record, Entry};
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

async fn superuser(db: &Db) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    svc.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Acme",
        last_name: None,
        password: "acmepw12345",
        timezone: None,
    })
    .await
    .unwrap();
    svc.authenticate("root", "acmepw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
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

async fn get(db: &Db, token: &str, path: &str) -> (StatusCode, String) {
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
async fn a_logged_error_is_listed_and_opens_to_read() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    record(
        &db,
        &Entry {
            level: "error",
            target: "acme::orders".into(),
            message: "could not charge the card".into(),
            detail: "order: 42\nat src/orders.rs:17".into(),
        },
    )
    .await
    .unwrap();

    let (status, list) = get(&db, &token, "/admin/event-log").await;
    assert_eq!(status, StatusCode::OK);
    assert!(list.contains("could not charge the card"), "{list}");
    assert!(list.contains(">Error<"), "the level is labelled: {list}");
    let at = list
        .find(r#"data-lat-href="/admin/event-log/"#)
        .expect("a row that opens");
    let href = &list[at + 15..at + 15 + list[at + 15..].find('"').unwrap()];
    assert!(href.ends_with("/preview"), "{href}");

    let (status, page) = get(&db, &token, href).await;
    assert_eq!(status, StatusCode::OK, "{href}");
    assert!(page.contains("order: 42"), "{page}");
    assert!(page.contains("acme::orders"));
    // Nothing edits a log row: the page offers no Edit, and no route takes one.
    let id = href
        .trim_end_matches("/preview")
        .rsplit('/')
        .next()
        .unwrap();
    assert!(
        !page.contains(&format!(r#"href="/admin/event-log/{id}/edit""#)),
        "{page}"
    );
    assert!(!list.contains("lat-rowactions__edit"), "{list}");
    let (status, _) = get(&db, &token, &format!("/admin/event-log/{id}/edit")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&db, &token, "/admin/event-log/new").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // It is listed under Logs in Settings.
    let (_, settings) = get(&db, &token, "/admin/settings").await;
    assert!(settings.contains("/admin/event-log"), "{settings}");
}
