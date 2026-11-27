//! The roles permission editor, end to end through the router: a form POST with
//! repeated `perm` fields persists the selected permissions as an array, and
//! only registered permissions are kept.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
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
            Contributions::default(),
            AdminConfig::default(),
            Arc::new(CatalogStore::default()),
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

async fn get_page(app: axum::Router, path: &str, token: &str) -> String {
    let resp = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("{SESSION_COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

/// One role's stored value of `column`.
async fn stored(pool: &Db, code: &str, column: &str) -> Option<String> {
    let (sql, values) = laterite_core::query::build(
        pool.backend,
        sea_query::Query::select()
            .column(sea_query::Alias::new(column))
            .from(sea_query::Alias::new("backend_roles"))
            .and_where(sea_query::Expr::col(sea_query::Alias::new("code")).eq(code))
            .to_owned(),
    );
    let row = laterite_core::query::bind_values(sqlx::query(&sql), values)
        .fetch_one(&pool.pool)
        .await
        .unwrap();
    laterite_core::AnyRowExt::get_text_opt(&row, column).unwrap()
}

async fn root_session(pool: &Db) -> String {
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
    root
}

#[tokio::test]
async fn a_role_carries_a_description_and_a_colour() {
    let (pool, _guard) = test_db().await;
    let root = root_session(&pool).await;
    let app = || {
        router(
            AuthService::new(pool.clone(), AuthConfig::default()),
            pool.clone(),
            Contributions::default(),
            AdminConfig::default(),
            Arc::new(CatalogStore::default()),
        )
    };
    let body = "code=writers&name=Writers&description=Drafts+and+edits+posts\
                &has_colour=1&colour=%232B7A4B&perm=backend.manage_users";
    let status = app()
        .oneshot(post_form("/admin/roles/new", &root, body))
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        stored(&pool, "writers", "description").await.as_deref(),
        Some("Drafts and edits posts")
    );
    assert_eq!(
        stored(&pool, "writers", "color").await.as_deref(),
        Some("#2b7a4b")
    );

    // The list shows both; the form brings them back.
    let list = get_page(app(), "/admin/roles", &root).await;
    assert!(list.contains("Drafts and edits posts"));
    assert!(list.contains("class=\"lat-swatch\" style=\"background:#2b7a4b\""));
    let id = laterite_auth::store::list_roles(&pool)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.code == "writers")
        .unwrap()
        .id;
    let form = get_page(app(), &format!("/admin/roles/{id}/edit"), &root).await;
    assert!(form.contains(">Drafts and edits posts</textarea>"));
    assert!(form.contains("value=\"#2b7a4b\""));

    // Unticking the box clears the colour; a value that is no colour is not kept.
    let path: &'static str = Box::leak(format!("/admin/roles/{id}/edit").into_boxed_str());
    app()
        .oneshot(post_form(
            path,
            &root,
            "code=writers&name=Writers&description=&colour=%232B7A4B",
        ))
        .await
        .unwrap();
    assert_eq!(stored(&pool, "writers", "color").await, None);
    assert_eq!(stored(&pool, "writers", "description").await, None);
    app()
        .oneshot(post_form(
            path,
            &root,
            "code=writers&name=Writers&has_colour=1&colour=red%3Bposition%3Afixed",
        ))
        .await
        .unwrap();
    assert_eq!(stored(&pool, "writers", "color").await, None);
}

#[tokio::test]
async fn the_editor_orders_explains_and_nests_permissions() {
    use laterite_admin::Permission;
    use laterite_core::t;
    let (pool, _guard) = test_db().await;
    let root = root_session(&pool).await;
    let app = || {
        router(
            AuthService::new(pool.clone(), AuthConfig::default()),
            pool.clone(),
            Contributions {
                permissions: vec![
                    Permission::new("acme.posts.publish", t!("Publish posts"), t!("Acme"))
                        .comment(t!("Makes a post public.")),
                    Permission::new("acme.posts", t!("Manage posts"), t!("Acme")).order(2),
                    Permission::new("acme.pages", t!("Manage pages"), t!("Acme")).order(1),
                ],
                ..Default::default()
            },
            AdminConfig::default(),
            Arc::new(CatalogStore::default()),
        )
    };
    let form = get_page(app(), "/admin/roles/new", &root).await;
    let at = |needle: &str| {
        form.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    // By order, with the nested one right after the one it sits under.
    assert!(at("value=\"acme.pages\"") < at("value=\"acme.posts\""));
    assert!(at("value=\"acme.posts\"") < at("value=\"acme.posts.publish\""));
    assert!(form.contains("data-lat-needs=\"acme.posts\""));
    assert!(form.contains("Makes a post public."));

    // A nested permission is kept only with the one it sits under.
    app()
        .oneshot(post_form(
            "/admin/roles/new",
            &root,
            "code=loose&name=Loose&perm=acme.posts.publish&perm=acme.pages",
        ))
        .await
        .unwrap();
    assert_eq!(
        stored(&pool, "loose", "permissions").await.as_deref(),
        Some(r#"["acme.pages"]"#)
    );
    app()
        .oneshot(post_form(
            "/admin/roles/new",
            &root,
            "code=whole&name=Whole&perm=acme.posts&perm=acme.posts.publish",
        ))
        .await
        .unwrap();
    assert_eq!(
        stored(&pool, "whole", "permissions").await.as_deref(),
        Some(r#"["acme.posts","acme.posts.publish"]"#)
    );
}
