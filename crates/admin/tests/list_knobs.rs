//! The knobs a resource sets on its list: the empty-state message and the
//! search box. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::list::{ListColumn, ListConfig, SearchConfig};
use laterite_admin::{router, AdminConfig, Contributions, Resource};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

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
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

/// A list over a table the built-in migrations create and nothing seeds.
fn things() -> ListConfig {
    ListConfig::new(
        "backend_roles",
        "Things",
        vec![ListColumn::new("name", "Name")],
    )
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

async fn page(router: Router, token: &str) -> (StatusCode, String) {
    page_at(router, "/admin/things", token).await
}

#[tokio::test]
async fn the_empty_message_is_the_descriptors() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().no_records_message("Nothing here yet.");
    let (status, html) = page(app(db, list), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Nothing here yet."));
    assert!(!html.contains("No records yet."));
}

#[tokio::test]
async fn the_default_empty_message_stands_when_none_is_set() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, html) = page(app(db, things()), &token).await;
    assert!(html.contains("No records yet."));
}

#[tokio::test]
async fn the_search_prompt_is_the_descriptors() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().search(SearchConfig::default().prompt("Find a thing"));
    let (_, html) = page(app(db, list), &token).await;
    assert!(html.contains("placeholder='Find a thing'"));
}

#[tokio::test]
async fn search_off_removes_the_box() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, html) = page(app(db, things().search(SearchConfig::off())), &token).await;
    assert!(!html.contains("id=\"lat-list-search\""));
}

#[tokio::test]
async fn search_on_enter_does_not_ask_while_typing() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().search(SearchConfig::default().on_enter());
    let (_, html) = page(app(db, list), &token).await;
    assert!(html.contains("id=\"lat-list-search\""));
    assert!(!html.contains("input changed delay"));
}

#[tokio::test]
async fn search_is_on_by_default() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, html) = page(app(db, things()), &token).await;
    assert!(html.contains("id=\"lat-list-search\""));
    assert!(html.contains("input changed delay"));
}

async fn post_setup(router: Router, token: &str, body: &str) -> StatusCode {
    router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/things/columns")
                .header("cookie", format!("{SESSION_COOKIE}={token}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn csrf_token(router: Router, token: &str) -> String {
    let (_, html) = page(router, token).await;
    let start = html.find("name=\"_csrf\" value=\"").unwrap() + 20;
    html[start..start + html[start..].find('"').unwrap()].to_string()
}

#[tokio::test]
async fn the_chosen_page_size_is_remembered_in_the_list_setup() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().per_page_options(vec![25, 50]);
    let csrf = csrf_token(app(db.clone(), list.clone()), &token).await;
    let status = post_setup(
        app(db.clone(), list.clone()),
        &token,
        &format!("_csrf={csrf}&column=name&per_page=50"),
    )
    .await;
    assert!(status.is_redirection() || status.is_success(), "{status}");
    let (_, html) = page(app(db, list), &token).await;
    assert!(html.contains("Rows per page"));
    assert!(
        html.contains("<option value=\"50\" selected>"),
        "the choice is shown selected"
    );
    assert!(!html.contains("per_page=50"), "and travels on no link");
}

#[tokio::test]
async fn a_size_not_offered_is_not_stored() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().per_page_options(vec![25, 50]);
    let csrf = csrf_token(app(db.clone(), list.clone()), &token).await;
    post_setup(
        app(db.clone(), list.clone()),
        &token,
        &format!("_csrf={csrf}&column=name&per_page=999"),
    )
    .await;
    let (_, html) = page(app(db, list), &token).await;
    assert!(
        html.contains("<option value=\"25\" selected>"),
        "the default stands"
    );
}

#[tokio::test]
async fn no_offered_sizes_means_no_chooser() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, html) = page(app(db, things()), &token).await;
    assert!(!html.contains("Rows per page"));
}

#[tokio::test]
async fn the_new_button_links_under_the_admin_mount_once() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().edit_base("/things").creatable();
    let (_, html) = page(app(db.clone(), list.clone()), &token).await;
    assert!(html.contains("href=\"/admin/things/new\""), "{html}");
    assert!(!html.contains("/admin/admin/"));
}
