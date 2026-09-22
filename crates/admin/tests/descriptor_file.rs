//! A resource written as a file, mounted and served like any other. Imports
//! only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const COOKIE: &str = "laterite_session";

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

fn app(db: Db) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db,
        Contributions {
            resources: vec![resource!("tests/descriptors/posts.yaml")],
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

async fn get(db: Db, path: &str, token: &str) -> (StatusCode, String) {
    let resp = app(db)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("{COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, body)
}

/// The whole point: a file, embedded at build time, serves a working screen.
#[tokio::test]
async fn a_resource_from_a_file_serves_its_list() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (status, html) = get(db, "/admin/posts", &token).await;
    assert_eq!(status, StatusCode::OK);

    assert!(html.contains("Posts"), "the title");
    assert!(html.contains("Title"), "a column labelled by the file");
    assert!(
        html.contains("Created at"),
        "and one labelled from its name"
    );
    assert!(html.contains("No posts yet."), "the empty message");
    assert!(
        html.contains("placeholder='Search posts'"),
        "the search prompt"
    );
    assert!(html.contains("Rows per page"), "the offered page sizes");
    assert!(html.contains("lat-col--right"), "the column alignment");
    assert!(html.contains("style=\"width:12%\""), "and its width");
}

#[tokio::test]
async fn the_form_the_file_describes_is_mounted() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (status, html) = get(db, "/admin/posts/new", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"name="code""#));
    assert!(html.contains(r#"name="name""#));
    assert!(html.contains("Title"), "a field labelled by the file");
}

/// A file cannot say one thing to the list and another to the form.
#[tokio::test]
async fn both_screens_read_the_same_entity() {
    let file = resource!("tests/descriptors/posts.yaml");
    assert_eq!(file.list.entity, "backend_roles");
    assert_eq!(file.form.as_ref().unwrap().entity, "backend_roles");
    assert_eq!(file.form.as_ref().unwrap().base_path, "/posts");
    assert_eq!(file.list.edit_base.as_deref(), Some("/posts"));
}
