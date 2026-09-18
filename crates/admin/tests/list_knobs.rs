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

#[tokio::test]
async fn offered_page_sizes_are_links_and_the_chosen_one_travels() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().per_page_options(vec![25, 50]);
    let (_, html) = page_at(app(db, list), "/admin/things?per_page=50", &token).await;
    assert!(html.contains("Per page"));
    assert!(html.contains("<b>50</b>"), "the chosen size is marked");
    assert!(
        html.contains("?per_page=25&amp;sort="),
        "the other size is a link"
    );
    assert!(
        // The escaper writes the carry's `&` as `&#38;`, the template's own as `&amp;`.
        html.replace("&#38;", "&amp;")
            .contains("&amp;per_page=50\""),
        "sort and pager links carry the choice"
    );
}

#[tokio::test]
async fn a_size_not_offered_falls_back_to_the_default() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let list = things().per_page_options(vec![25, 50]);
    let (_, html) = page_at(app(db, list), "/admin/things?per_page=999", &token).await;
    assert!(html.contains("<b>25</b>"));
    assert!(!html.contains("per_page=999"));
}

#[tokio::test]
async fn no_offered_sizes_means_no_chooser() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (_, html) = page_at(app(db, things()), "/admin/things?per_page=50", &token).await;
    assert!(!html.contains("Per page"));
    assert!(
        !html.contains("per_page=50"),
        "an unoffered size is ignored, not carried"
    );
}
