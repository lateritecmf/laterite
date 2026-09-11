//! A module mounting its own routes: an admin screen and a public endpoint.
//! Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use laterite_admin::routes::{PublicRoute, PublicRouteReg, RouteCtx, Screen, ScreenReg};
use laterite_admin::{router, AdminConfig, Contributions};
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
        Contributions {
            screens: vec![
                ScreenReg::new("/import", PERMISSION, Arc::new(Importer)).in_menu("Import places"),
                ScreenReg::new("/hidden", PERMISSION, Arc::new(Importer)),
            ],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

fn app(db: Db, base: &str) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    router(
        auth,
        db,
        Contributions {
            screens: vec![ScreenReg::new(base, PERMISSION, Arc::new(Importer))],
            ..Default::default()
        },
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

/// A module's own extension point: a type the framework knows nothing about,
/// which other modules contribute and this module's route reads.
struct FeedReg {
    name: &'static str,
}

/// A public endpoint that reads what other modules contributed to it, and the
/// site's own origin, which is what a sitemap or a feed needs.
struct Feeds;

impl PublicRoute for Feeds {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let mut names: Vec<&str> = ctx
            .contributions::<FeedReg>()
            .iter()
            .map(|r| r.name)
            .collect();
        names.sort_unstable();
        let listed = names.join(",");
        let origin = ctx.base_url().to_string();
        Router::new()
            .route("/", get(move || async move { listed }))
            .route("/origin", get(move || async move { origin }))
    }
}

fn app_with_plugin_defined(db: Db) -> Router {
    app_with_origin(db, "")
}

fn app_with_origin(db: Db, origin: &str) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    // AdminConfig is non-exhaustive, so it is built rather than literalled.
    let mut config = AdminConfig::default();
    config.origin = origin.to_string();
    // Two modules contribute a type only they know about; a third reads it.
    let mut registry = laterite_core::Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("acme.blog"));
    registry.add(FeedReg { name: "posts" });
    registry.set_owner(laterite_core::ModuleId::new("acme.shop"));
    registry.add(FeedReg { name: "products" });

    router(
        auth,
        db,
        Contributions {
            public_routes: vec![PublicRouteReg::new("/feeds", Arc::new(Feeds))],
            plugin_defined: Arc::new(registry),
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

#[tokio::test]
async fn a_route_reads_contributions_the_framework_never_learns_about() {
    let (db, _guard) = test_db().await;
    let app = app_with_plugin_defined(db);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/feeds")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    // Both modules' contributions reached the reading module, whatever order
    // they registered in: everything is collected before any route runs.
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "posts,products");
}

#[tokio::test]
async fn a_route_is_told_the_sites_own_origin() {
    let (db, _guard) = test_db().await;
    // Configured with a trailing slash, which the admin trims: a `<loc>` built
    // from this must not end up with a doubled separator.
    let resp = app_with_origin(db, "https://acme.example/")
        .oneshot(
            Request::builder()
                .uri("/feeds/origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    // Absolute URLs come from configuration, never the request Host, which a
    // proxy controls.
    assert_eq!(
        String::from_utf8(body.to_vec()).unwrap(),
        "https://acme.example"
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
        Contributions {
            public_routes: vec![PublicRouteReg::new("/robots.txt", Arc::new(Robots))],
            ..Default::default()
        },
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

/// A public route is nested at its literal path, so a request *below* that path
/// enters the nested router and matches nothing there. What it must not do is
/// escape into axum's bare 404: the deployment's styled error page is the whole
/// point of routing these through the framework rather than beside it. Pinned
/// because fallback inheritance through `nest_service` is a behaviour, not a
/// guarantee we control.
#[tokio::test]
async fn a_path_below_a_public_route_still_gets_the_framework_404() {
    let (db, _guard) = test_db().await;
    superuser(&db).await;
    let resp = app_with_public(db)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/robots.txt/nonsense")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("<!DOCTYPE html>") || body.contains("<html"),
        "expected the styled error page, got: {body:?}"
    );
}

/// A plugin has to be able to exercise its own routes without booting the
/// framework, which is what the public builder is for. It also defaults the admin
/// path to `/admin`, so a route that excludes the panel must be tested against a
/// different one to prove it reads the value rather than hardcoding it.
#[tokio::test]
async fn a_route_can_be_mounted_from_a_hand_built_context() {
    let (db, _guard) = test_db().await;
    let ctx = RouteCtx::builder(db)
        .admin_path("/backoffice")
        .base_url("https://acme.example")
        .build();

    assert_eq!(ctx.admin_path(), "/backoffice");
    assert_eq!(ctx.base_url(), "https://acme.example");

    let resp = Robots
        .mount(&ctx)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/panel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), "/backoffice");
}
