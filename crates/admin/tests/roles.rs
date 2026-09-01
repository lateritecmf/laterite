//! The roles permission editor, end to end through the router: a form POST with
//! repeated `perm` fields persists the selected permissions as an array, and
//! only registered permissions are kept.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::Db;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

/// A fresh test database with the admin's built-in migrations applied, on
/// whichever backend the run targets. Hold the guard for the test's lifetime.
async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

async fn login(svc: &AuthService, username: &str, password: &str) -> String {
    svc.authenticate(username, password, &RequestContext::default())
        .await
        .expect("authenticate")
        .token
}

/// A known CSRF token the tests seed into the session (see [`seed_csrf`]).
const CSRF: &str = "itest-csrf";

/// Seeds the session with a known CSRF token so a POST can present it.
async fn seed_csrf(svc: &AuthService, token: &str) {
    svc.set_session_data(token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
        .await
        .unwrap();
}

/// A form POST carrying the session cookie plus the CSRF signals (same-origin
/// and the seeded token in the header).
fn post_form(path: &str, token: &str, body: &'static str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", CSRF)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn editor_saves_only_registered_permissions() {
    let (pool, _guard) = test_db().await;

    let svc = AuthService::new(pool.clone(), AuthConfig::default());
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
    let root = login(&svc, "root", "rootpw12345").await;
    seed_csrf(&svc, &root).await;

    let app = || {
        router(
            AuthService::new(pool.clone(), AuthConfig::default()),
            pool.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AdminConfig::default(),
        )
    };

    // Create a role with two valid grants and one that was never registered.
    let body = "code=editors&name=Editors\
                &perm=backend.manage_users&perm=nope.invalid&perm=backend.manage_roles";
    let status = app()
        .oneshot(post_form("/admin/roles/new", &root, body))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::SEE_OTHER);

    // The unregistered permission is dropped; the registered ones persist as a
    // JSON array of strings, in registry order. Read through the query layer so
    // the placeholder renders correctly on each backend.
    let (sql, values) = laterite_core::query::build(
        pool.backend,
        sea_query::Query::select()
            .column(sea_query::Alias::new("permissions"))
            .from(sea_query::Alias::new("backend_roles"))
            .and_where(sea_query::Expr::col(sea_query::Alias::new("code")).eq("editors"))
            .to_owned(),
    );
    let row = laterite_core::query::bind_values(sqlx::query(&sql), values)
        .fetch_one(&pool.pool)
        .await
        .unwrap();
    let saved_json = laterite_core::AnyRowExt::get_text(&row, "permissions").unwrap();
    let saved: Vec<String> = serde_json::from_str(&saved_json).unwrap();
    assert_eq!(saved, ["backend.manage_users", "backend.manage_roles"]);
}
