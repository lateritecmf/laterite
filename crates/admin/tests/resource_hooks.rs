//! A module changes another module's resource before it mounts: a column, a
//! menu placement, a field. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::list::ListColumn;
use laterite_admin::nav::Placement;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission, ResourceHook};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use sea_query::Alias;
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

struct CreatePosts;

#[async_trait(?Send)]
impl Migration for CreatePosts {
    fn name(&self) -> &str {
        "0001_create_posts"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("posts"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.posts", vec![Box::new(CreatePosts)]));
    laterite_core::testing::connect_test(&sets).await
}

fn contributions(hooks: Vec<ResourceHook>) -> Contributions {
    Contributions {
        resources: vec![resource!("tests/descriptors/form_handle.yaml")],
        permissions: vec![Permission::new(
            "acme.manage_posts",
            t!("Manage posts"),
            t!("Acme"),
        )],
        resource_hooks: hooks,
        ..Default::default()
    }
}

fn app(db: &Db, hooks: Vec<ResourceHook>) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        contributions(hooks),
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn session(db: &Db) -> String {
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
async fn a_hook_changes_a_resource_it_does_not_own_before_it_mounts() {
    let (db, _guard) = test_db().await;
    let hook = ResourceHook::new("/posts", |mut resource| {
        resource
            .list
            .columns
            .push(ListColumn::new("code", "Code word"));
        resource.menu(Placement::new().icon("map"))
    });
    let app = app(&db, vec![hook]);
    let session = session(&db).await;

    let (status, html) = get(&app, "/admin/posts", &session).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Code word"),
        "the added column heads the list: {html}"
    );
    assert!(
        html.contains(r#"href="/admin/posts""#) && html.contains("lat-topnav__ico"),
        "the placement took: {html}"
    );
}

#[tokio::test]
#[should_panic(expected = "which no module declared")]
async fn a_hook_naming_no_resource_aborts_the_boot() {
    let (db, _guard) = test_db().await;
    let hook = ResourceHook::new("/nowhere", |resource| resource);
    let _ = app(&db, vec![hook]);
}
