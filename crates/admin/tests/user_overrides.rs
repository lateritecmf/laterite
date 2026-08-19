//! Per-user permission overrides, end to end: an allow override grants access
//! beyond a user's roles, and the editor cannot change permissions it does not
//! itself hold (no privilege escalation).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::Db;
use std::collections::HashMap;
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

fn get(path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, token: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

async fn overrides_of(pool: &Db, user_id: &str) -> HashMap<String, i64> {
    // Ids are bigint auto-increment keys; permissions are a JSON text object.
    let json: String = sqlx::query_scalar("select permissions from backend_users where id = ?")
        .bind(user_id.parse::<i64>().unwrap())
        .fetch_one(&pool.pool)
        .await
        .unwrap();
    serde_json::from_str(&json).unwrap()
}

async fn plain_user(pool: &Db, name: &str) -> String {
    let hash = password::hash_password("pw012345678").unwrap();
    store::create_user(
        pool,
        name,
        &format!("{name}@acme.test"),
        name,
        None,
        &hash,
        false,
    )
    .await
    .unwrap()
    .to_string()
}

fn app(pool: &Db) -> axum::Router {
    router(
        AuthService::new(pool.clone(), AuthConfig::default()),
        pool.clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        AdminConfig::default(),
    )
}

async fn seed_superuser(svc: &AuthService) {
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
}

#[tokio::test]
async fn allow_override_grants_access_beyond_roles() {
    let pool = test_db().await;
    let svc = AuthService::new(pool.clone(), AuthConfig::default());
    seed_superuser(&svc).await;
    let root = login(&svc, "root", "rootpw12345").await;

    // A user with no roles cannot reach the users list.
    let user_id = plain_user(&pool, "grantee").await;
    let user = login(&svc, "grantee", "pw012345678").await;
    let status = app(&pool)
        .oneshot(get("/admin/users", &user))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The superuser grants an explicit allow override.
    let status = app(&pool)
        .oneshot(post(
            &format!("/admin/users/{user_id}/edit"),
            &root,
            "p:backend.manage_users=1".to_string(),
        ))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        overrides_of(&pool, &user_id)
            .await
            .get("backend.manage_users"),
        Some(&1)
    );

    // The same session now resolves the override and is admitted.
    let status = app(&pool)
        .oneshot(get("/admin/users", &user))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn editor_cannot_change_permissions_it_lacks() {
    let pool = test_db().await;
    let svc = AuthService::new(pool.clone(), AuthConfig::default());
    seed_superuser(&svc).await;

    // A manager holds backend.manage_users (via a role) but not manage_roles.
    let mgr_id = plain_user(&pool, "mgr").await;
    let role = store::create_role(
        &pool,
        "user_mgr",
        "User Manager",
        &["backend.manage_users".to_string()],
    )
    .await
    .unwrap();
    store::assign_role(&pool, mgr_id.parse().unwrap(), role)
        .await
        .unwrap();
    let mgr = login(&svc, "mgr", "pw012345678").await;

    // The manager edits another user, trying to grant a permission they do not
    // hold (manage_roles) and to deny one they do (manage_users).
    let target = plain_user(&pool, "target").await;
    let status = app(&pool)
        .oneshot(post(
            &format!("/admin/users/{target}/edit"),
            &mgr,
            "p:backend.manage_roles=1&p:backend.manage_users=-1".to_string(),
        ))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::SEE_OTHER);

    // Only the permission the manager holds was applied; the escalation attempt
    // on manage_roles was dropped.
    let saved = overrides_of(&pool, &target).await;
    assert_eq!(saved.get("backend.manage_users"), Some(&-1));
    assert_eq!(saved.get("backend.manage_roles"), None);
}
