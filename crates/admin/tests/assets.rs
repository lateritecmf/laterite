//! The embedded-asset route serves the built-in assets by path and 404s unknown
//! ones. Imports only the public API (the router).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig};
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::Db;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn serves_embedded_assets_and_404s_unknown() {
    let (pool, _guard) = test_db().await;
    let app = || {
        router(
            AuthService::new(pool.clone(), AuthConfig::default()),
            pool.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AdminConfig::default(),
        )
    };

    // The stylesheet serves with a css content type.
    let css = app()
        .oneshot(get("/admin/assets/laterite.css"))
        .await
        .unwrap();
    assert_eq!(css.status(), StatusCode::OK);
    let ct = css.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/css"), "content-type was {ct}");

    // A nested path (a webfont) serves through the wildcard.
    let font = app()
        .oneshot(get("/admin/assets/fonts/space-grotesk-500.woff2"))
        .await
        .unwrap();
    assert_eq!(font.status(), StatusCode::OK);

    // Vendored htmx serves through the same nested wildcard, as JavaScript.
    let htmx = app()
        .oneshot(get("/admin/assets/vendor/htmx.min.js"))
        .await
        .unwrap();
    assert_eq!(htmx.status(), StatusCode::OK);
    let ct = htmx
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("javascript"), "content-type was {ct}");

    // An unregistered path is a 404.
    let miss = app().oneshot(get("/admin/assets/nope.css")).await.unwrap();
    assert_eq!(miss.status(), StatusCode::NOT_FOUND);
}
