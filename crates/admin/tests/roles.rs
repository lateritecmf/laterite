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

/// A fresh in-memory SQLite database with the admin's built-in migrations
/// applied, the same runner path the application uses at startup.
async fn test_db() -> Db {
    sqlx::any::install_default_drivers();
    let pool = sqlx::any::AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool");
    let db = Db::new(pool, laterite_core::DbBackend::Sqlite);
    laterite_core::migration::run(&db.pool, db.backend, &laterite_admin::builtin_migrations())
        .await
        .expect("migrations should apply");
    db
}

async fn login(svc: &AuthService, username: &str, password: &str) -> String {
    svc.authenticate(username, password, &RequestContext::default())
        .await
        .expect("authenticate")
        .token
}

fn post_form(path: &str, token: &str, body: &'static str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn editor_saves_only_registered_permissions() {
    let pool = test_db().await;

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
    // JSON array of strings, in registry order.
    let saved_json: String =
        sqlx::query_scalar("select permissions from backend_roles where code = ?")
            .bind("editors")
            .fetch_one(&pool.pool)
            .await
            .unwrap();
    let saved: Vec<String> = serde_json::from_str(&saved_json).unwrap();
    assert_eq!(saved, ["backend.manage_users", "backend.manage_roles"]);
}
