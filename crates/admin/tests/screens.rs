//! A module mounting its own routes: an admin screen and a public endpoint.
//! Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use laterite_admin::routes::{PublicRoute, PublicRouteReg, RouteCtx, Screen, ScreenReg};
use laterite_admin::{router, AdminConfig};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const PERMISSION: &str = "acme.import";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// A screen with two routes, one of which reports the base it mounted at, so the
/// test can assert a self-link is built from the resolved path.
struct Importer;

impl Screen for Importer {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let here = ctx.url("/step2");
        Router::new()
            .route("/", get(|| async { "import" }))
            .route("/where", get(move || async move { here }))
    }
}

fn app_in_menu(db: Db) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    router(
        auth,
        db,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![
            ScreenReg::new("/import", PERMISSION, Arc::new(Importer)).in_menu("Import places"),
            ScreenReg::new("/hidden", PERMISSION, Arc::new(Importer)),
        ],
        Vec::new(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

fn app(db: Db, base: &str) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    router(
        auth,
        db,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![ScreenReg::new(base, PERMISSION, Arc::new(Importer))],
        Vec::new(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

async fn body(router: Router, path: &str, token: &str) -> (StatusCode, String) {
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

/// A superuser (who holds every permission) and their session token.
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

#[tokio::test]
async fn a_contributed_screen_serves_under_the_admin_mount() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (status, body) = body(app(db, "/import"), "/admin/import", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "import");
}

#[tokio::test]
async fn a_screen_links_to_itself_through_its_resolved_base() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    // The screen never sees a literal path: it asks the context where it is.
    let (_, body) = body(app(db, "/import"), "/admin/import/where", &token).await;
    assert_eq!(body, "/admin/import/step2");
}

#[tokio::test]
async fn the_framework_enforces_the_screens_permission() {
    let (db, _guard) = test_db().await;
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    // A superuser exists so the panel is past first-run setup.
    superuser(&db).await;
    let hash = password::hash_password("editorpw12345").unwrap();
    store::create_user(
        &db,
        "editor",
        "editor@acme.test",
        "Editor",
        None,
        &hash,
        false,
    )
    .await
    .unwrap();
    let token = svc
        .authenticate("editor", "editorpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;

    // The screen's own handler never checks anything; the guard runs first.
    let (status, _) = body(app(db, "/import"), "/admin/import", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_unauthenticated_request_never_reaches_the_screen() {
    let (db, _guard) = test_db().await;
    superuser(&db).await;
    let resp = app(db, "/import")
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/import")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "redirected to sign in"
    );
}

/// A public endpoint: no session, no permission, a literal path.
struct Robots;

impl PublicRoute for Robots {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        // A module asks where the panel is rather than assuming /admin.
        let panel = ctx.admin_path().to_string();
        Router::new()
            .route("/", get(|| async { "User-agent: *\nAllow: /\n" }))
            .route("/panel", get(move || async move { panel }))
    }
}

fn app_with_public(db: Db) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    router(
        auth,
        db,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![PublicRouteReg::new("/robots.txt", Arc::new(Robots))],
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

#[tokio::test]
async fn a_public_route_answers_without_a_session() {
    let (db, _guard) = test_db().await;
    superuser(&db).await;
    let resp = app_with_public(db)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/robots.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8(bytes.to_vec())
        .unwrap()
        .contains("User-agent"));
}

#[tokio::test]
async fn a_public_route_does_not_disturb_the_admin() {
    let (db, _guard) = test_db().await;
    superuser(&db).await;
    // An unmatched URL still renders the styled 404 rather than the public route.
    let resp = app_with_public(db)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/nothing-here")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_module_can_ask_where_the_panel_is() {
    let (db, _guard) = test_db().await;
    superuser(&db).await;
    let resp = app_with_public(db)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/robots.txt/panel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), "/admin");
}

#[tokio::test]
async fn a_screen_appears_in_the_menu_only_when_it_asks() {
    let (db, _guard) = test_db().await;
    let token = superuser(&db).await;
    let (status, html) = body(app_in_menu(db), "/admin", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Import places"),
        "the labelled screen is listed"
    );
    assert!(html.contains("/admin/import"), "and links to itself");
    assert!(
        !html.contains("/admin/hidden"),
        "a screen with no label stays out of the menu"
    );
}
