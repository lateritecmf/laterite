//! A module mounting its own admin screen. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use laterite_admin::screen::{Screen, ScreenCtx, ScreenReg};
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
    fn mount(&self, ctx: &ScreenCtx) -> Router {
        let here = ctx.url("/step2");
        Router::new()
            .route("/", get(|| async { "import" }))
            .route("/where", get(move || async move { here }))
    }
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
