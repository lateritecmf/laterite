//! What a list does with no configuration: rows open their record, a stored
//! code shows its label, and the query an operator used comes back. Imports
//! only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
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
        first_name: "Root",
        last_name: None,
        password: "rootpw12345",
        timezone: None,
    })
    .await
    .unwrap();
    svc.authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

fn app(db: &Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![
                resource!("tests/descriptors/list_behaviours.yaml"),
                resource!("tests/descriptors/list_plain.yaml"),
            ],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn get(db: &Db, token: &str, path: &str) -> (StatusCode, Option<String>, String) {
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
    let location = resp
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, location, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn a_row_opens_its_record_and_a_code_shows_its_label() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let admin = store::create_role(&db, "admin", "Admin", &[])
        .await
        .unwrap();
    store::create_role(&db, "guest", "Guest", &[])
        .await
        .unwrap();

    let (status, _, html) = get(&db, &token, "/admin/teams").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(&format!(
            r#"data-lat-href="/admin/teams/{admin}/edit" tabindex="0""#
        )),
        "{html}"
    );
    assert!(
        html.contains("Administrators"),
        "the code is labelled: {html}"
    );
    assert!(!html.contains(">admin<"), "and not shown raw: {html}");
    assert!(
        html.contains(">guest<"),
        "a value with no label is shown as stored: {html}"
    );

    let (_, _, plain) = get(&db, &token, "/admin/plain").await;
    assert!(!plain.contains("data-lat-href"), "row_click: none: {plain}");
    assert!(plain.contains("/edit\">"), "the Edit link stays");
}

#[tokio::test]
async fn the_query_an_operator_used_comes_back() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    store::create_role(&db, "admin", "Admin", &[])
        .await
        .unwrap();

    // Nothing remembered yet: the plain list renders.
    let (status, _, _) = get(&db, &token, "/admin/teams").await;
    assert_eq!(status, StatusCode::OK);

    // A search is remembered; a plain return is sent back to it.
    let (status, _, _) = get(&db, &token, "/admin/teams?q=adm&page=2&sort=code&dir=asc").await;
    assert_eq!(status, StatusCode::OK);
    let (status, location, _) = get(&db, &token, "/admin/teams").await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(
        location.as_deref(),
        Some("/admin/teams?sort=code&dir=asc&q=adm"),
        "the sort and search, not the page"
    );

    // A swap names what it wants and is never redirected.
    let resp = app(&db)
        .oneshot(
            Request::builder()
                .uri("/admin/teams")
                .header("cookie", format!("laterite_session={token}"))
                .header("hx-request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Clearing the search forgets it.
    let (status, _, _) = get(&db, &token, "/admin/teams?q=&sort=id&dir=desc").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get(&db, &token, "/admin/teams").await;
    assert_eq!(status, StatusCode::OK, "nothing to bring back");

    // A list that declined remembers nothing.
    let (status, _, _) = get(&db, &token, "/admin/plain?q=adm").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get(&db, &token, "/admin/plain").await;
    assert_eq!(status, StatusCode::OK);
}
