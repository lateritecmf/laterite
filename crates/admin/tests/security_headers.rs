//! Every admin response carries the browser-side hardening headers, and the
//! transport header only where the deployment says it serves over TLS.
//! Imports only the public API (the router).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig, Contributions, CONTENT_SECURITY_POLICY};
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

fn header<'a>(res: &'a axum::response::Response, name: &str) -> Option<&'a str> {
    res.headers().get(name).and_then(|v| v.to_str().ok())
}

#[tokio::test]
async fn pages_carry_the_hardening_headers() {
    let (pool, _guard) = test_db().await;
    let app = router(
        AuthService::new(pool.clone(), AuthConfig::default()),
        pool.clone(),
        Contributions::default(),
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
    let res = app.oneshot(get("/admin/login")).await.unwrap();
    // A fresh database answers with the setup redirect; the headers ride on
    // every response whatever its status.
    assert!(res.status() == StatusCode::OK || res.status().is_redirection());
    assert_eq!(
        header(&res, "content-security-policy"),
        Some(CONTENT_SECURITY_POLICY)
    );
    assert!(CONTENT_SECURITY_POLICY.contains("script-src 'self';"));
    assert!(!CONTENT_SECURITY_POLICY.contains("script-src 'self' 'unsafe-inline'"));
    assert_eq!(header(&res, "x-content-type-options"), Some("nosniff"));
    assert_eq!(header(&res, "x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(header(&res, "referrer-policy"), Some("same-origin"));
    assert_eq!(
        header(&res, "cross-origin-opener-policy"),
        Some("same-origin")
    );
    assert!(header(&res, "permissions-policy").is_some());
    // A plain development server never pins browsers to HTTPS.
    assert!(header(&res, "strict-transport-security").is_none());
}

#[tokio::test]
async fn a_tls_deployment_pins_the_transport() {
    let (pool, _guard) = test_db().await;
    let mut config = AdminConfig::default();
    config.secure_cookie = true;
    let app = router(
        AuthService::new(pool.clone(), AuthConfig::default()),
        pool.clone(),
        Contributions::default(),
        config,
        Arc::new(CatalogStore::default()),
    );
    let res = app.oneshot(get("/admin/login")).await.unwrap();
    assert_eq!(
        header(&res, "strict-transport-security"),
        Some("max-age=31536000; includeSubDomains")
    );
}
