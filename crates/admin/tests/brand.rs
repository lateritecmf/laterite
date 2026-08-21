//! The configured application name renders as the admin brand, and a brand
//! setting overrides it. Imports only the framework's public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::settings::{save, BrandSetting};
use laterite_admin::{router, AdminConfig};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::Db;
use tower::ServiceExt;

/// A fresh test database with the admin's built-in migrations applied. Hold the
/// guard for the test's lifetime.
async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// The admin router with a configured application name and nothing else.
fn app(db: Db, app_name: &str) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    router(
        auth,
        db,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        AdminConfig {
            secure_cookie: false,
            timezone: "UTC".to_string(),
            app_name: app_name.to_string(),
        },
    )
}

/// GETs a path and returns the status and body. A fresh install (no operators)
/// serves the first-run setup page at `/admin/setup`, which renders the brand.
async fn get(router: Router, path: &str) -> (StatusCode, String) {
    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
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
async fn configured_app_name_is_the_brand() {
    let (db, _guard) = test_db().await;
    let (status, html) = get(app(db, "Acme Blog"), "/admin/setup").await;
    assert_eq!(status, StatusCode::OK);
    // The brand shows in the heading; the hardcoded "Laterite" wordmark is gone.
    assert!(html.contains("Acme Blog"), "shows the configured app name");
    assert!(!html.contains("<h1>Laterite</h1>"), "no hardcoded brand");
}

#[tokio::test]
async fn branding_form_prefills_the_configured_name() {
    let (db, _guard) = test_db().await;

    // A superuser to sign in with (the branding form is a protected route).
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    auth.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Root",
        last_name: None,
        password: "rootpw12345",
        timezone: None,
    })
    .await
    .unwrap();
    let token = auth
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;

    // Open the branding settings form with no brand setting saved yet.
    let resp = app(db, "Configured Name")
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin/settings/laterite.brand")
                .header("cookie", format!("laterite_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    // The application-name field is prefilled with the configured name rather
    // than opening blank.
    assert!(
        html.contains(r#"value="Configured Name""#),
        "branding form seeds the configured app name into the field"
    );
}

#[tokio::test]
async fn brand_setting_overrides_the_configured_name() {
    let (db, _guard) = test_db().await;
    save(
        &db,
        &BrandSetting {
            app_name: "Override Co".to_string(),
        },
    )
    .await
    .unwrap();

    let (status, html) = get(app(db, "Acme Blog"), "/admin/setup").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Override Co"), "the setting overrides config");
    assert!(!html.contains("Acme Blog"), "the config name is overridden");
}
