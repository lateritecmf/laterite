//! Route-level permission enforcement for built-in resources. Imports only the
//! framework's public API: the admin router and the auth service and stores.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::Db;
use tower::ServiceExt;

/// The session cookie name the admin sets and reads (wire format).
const SESSION_COOKIE: &str = "laterite_session";

/// A fresh test database with the admin's built-in migrations applied, on
/// whichever backend the run targets. Hold the guard for the test's lifetime.
async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

/// Signs in and returns the session token.
async fn login(svc: &AuthService, username: &str, password: &str) -> String {
    svc.authenticate(username, password, &RequestContext::default())
        .await
        .expect("authenticate")
        .token
}

/// A GET request for `path`, optionally carrying a session cookie.
fn get(path: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(token) = token {
        builder = builder.header("cookie", format!("{SESSION_COOKIE}={token}"));
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn resource_routes_enforce_their_permission() {
    let (pool, _guard) = test_db().await;

    let svc = AuthService::new(pool.clone(), AuthConfig::default());

    // A superuser (as the first-run flow creates): passes every check.
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

    // A user granted `backend.manage_users` through a role, but not
    // `backend.manage_roles`.
    let hash = password::hash_password("mgrpw12345").unwrap();
    let mgr_id = store::create_user(&pool, "mgr", "mgr@acme.test", "Mgr", None, &hash, false)
        .await
        .unwrap();
    let role_id = store::create_role(
        &pool,
        "user_mgr",
        "User Manager",
        &["backend.manage_users".to_string()],
    )
    .await
    .unwrap();
    store::assign_role(&pool, mgr_id, role_id).await.unwrap();

    // A user granted only `backend.manage_plugins`, to prove that route is gated
    // by its own permission and not by the users/roles grants.
    let hash = password::hash_password("plugpw123456").unwrap();
    let plug_id = store::create_user(&pool, "plug", "plug@acme.test", "Plug", None, &hash, false)
        .await
        .unwrap();
    let plug_role = store::create_role(
        &pool,
        "plugin_mgr",
        "Plugin Manager",
        &["backend.manage_plugins".to_string()],
    )
    .await
    .unwrap();
    store::assign_role(&pool, plug_id, plug_role).await.unwrap();

    // A user with no grants at all.
    let hash = password::hash_password("plainpw12345").unwrap();
    store::create_user(
        &pool,
        "plain",
        "plain@acme.test",
        "Plain",
        None,
        &hash,
        false,
    )
    .await
    .unwrap();

    let root = login(&svc, "root", "rootpw12345").await;
    let mgr = login(&svc, "mgr", "mgrpw12345").await;
    let plug = login(&svc, "plug", "plugpw123456").await;
    let plain = login(&svc, "plain", "plainpw12345").await;

    // A fresh router per request: `oneshot` consumes the service.
    let app = || {
        router(
            AuthService::new(pool.clone(), AuthConfig::default()),
            pool.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AdminConfig::default(),
        )
    };

    // The superuser and the granted operator reach the users list.
    let status = app()
        .oneshot(get("/admin/users", Some(&root)))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);
    let status = app()
        .oneshot(get("/admin/users", Some(&mgr)))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);

    // The operator without the grant is refused the users list.
    let status = app()
        .oneshot(get("/admin/users", Some(&plain)))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The grant is per resource: the user manager lacks `backend.manage_roles`,
    // so the roles list is refused even though the users list is allowed.
    let status = app()
        .oneshot(get("/admin/roles", Some(&mgr)))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The plugins screen has its own permission: its holder reaches it, the user
    // manager (a different grant) does not, and the superuser always does.
    let status = app()
        .oneshot(get("/admin/plugins", Some(&plug)))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);
    let status = app()
        .oneshot(get("/admin/plugins", Some(&mgr)))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let status = app()
        .oneshot(get("/admin/plugins", Some(&root)))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);

    // An unauthenticated request is sent to login, not forbidden.
    let status = app()
        .oneshot(get("/admin/users", None))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::SEE_OTHER);
}
