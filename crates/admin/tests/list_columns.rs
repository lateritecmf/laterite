//! The knobs a resource sets on a column: sortable, invisible, width, align and
//! permission. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::list::{Align, ListColumn, ListConfig};
use laterite_admin::{router, AdminConfig, Contributions, Permission, Resource};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const SECRET: &str = "acme.see_secret";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: Db, list: ListConfig) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    router(
        auth,
        db,
        Contributions {
            resources: vec![Resource::new("/things", "Things", list)],
            permissions: vec![Permission::new(
                SECRET,
                t!("See the secret column"),
                t!("Things"),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

/// A list over a table the built-in migrations create and nothing seeds.
fn things(columns: Vec<ListColumn>) -> ListConfig {
    ListConfig::new("backend_roles", "Things", columns)
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
    token(db, "root", "rootpw12345").await
}

/// An operator holding no permission at all.
async fn nobody(db: &Db) -> String {
    let hash = password::hash_password("nobodypw12345").unwrap();
    store::create_user(db, "nobody", "nobody@acme.test", "No", None, &hash, false)
        .await
        .unwrap();
    token(db, "nobody", "nobodypw12345").await
}

async fn token(db: &Db, username: &str, password: &str) -> String {
    AuthService::new(db.clone(), AuthConfig::default())
        .authenticate(username, password, &RequestContext::default())
        .await
        .unwrap()
        .token
}

async fn page_at(router: Router, path: &str, token: &str) -> (StatusCode, String) {
    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header("cookie", format!("{SESSION_COOKIE}={token}"))
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
async fn an_unsortable_column_has_no_sort_link_and_ignores_the_parameter() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things(vec![
        ListColumn::new("code", "Code"),
        ListColumn::new("name", "Name").sortable(false),
    ]);
    let (_, html) = page_at(app(db, list), "/admin/things?sort=name&dir=asc", &token).await;
    assert!(html.contains("?sort=code&amp;"), "a sortable column links");
    assert!(
        !html.contains("?sort=name&amp;"),
        "an unsortable one does not"
    );
    assert!(
        !html.contains("aria-sort="),
        "the ignored sort marks nothing active"
    );
}

#[tokio::test]
async fn an_invisible_column_is_hidden_by_default_and_offered_in_the_picker() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things(vec![
        ListColumn::new("code", "Code"),
        ListColumn::new("name", "Name").invisible(),
    ]);
    let (_, html) = page_at(app(db, list), "/admin/things", &token).await;
    assert!(html.contains("?sort=code&amp;"));
    assert!(!html.contains("?sort=name&amp;"), "hidden until chosen");
    assert!(
        html.contains("value=\"name\""),
        "but offered in the column picker"
    );
}

#[tokio::test]
async fn width_and_alignment_reach_the_header() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things(vec![ListColumn::new("code", "Code")
        .width("10%")
        .align(Align::Right)]);
    let (_, html) = page_at(app(db, list), "/admin/things", &token).await;
    assert!(html.contains("style=\"width:10%\""));
    assert!(html.contains("lat-col--right"));
}

#[tokio::test]
async fn a_gated_column_is_hidden_from_an_operator_without_the_permission() {
    let (db, _guard) = test_db().await;
    let root = superuser(&db).await;
    let nobody = nobody(&db).await;
    let list = things(vec![
        ListColumn::new("code", "Code"),
        ListColumn::new("name", "Name").require(SECRET),
    ])
    .exportable();

    let (_, html) = page_at(app(db.clone(), list.clone()), "/admin/things", &root).await;
    assert!(html.contains("?sort=name&amp;"), "a superuser sees it");

    let (_, html) = page_at(app(db.clone(), list.clone()), "/admin/things", &nobody).await;
    assert!(
        !html.contains("?sort=name&amp;"),
        "an operator without the grant does not"
    );
    assert!(
        !html.contains("value=\"name\""),
        "and cannot pick it either"
    );

    let (_, csv) = page_at(app(db, list), "/admin/things/export?format=csv", &nobody).await;
    assert!(
        csv.starts_with("Code\n") || csv.starts_with("Code\r\n"),
        "export leaks nothing: {csv:?}"
    );
}
