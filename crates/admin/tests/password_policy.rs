//! The password policy on the admin's own screens: every rule a new password
//! breaks is named, an old password must be replaced before anything else
//! opens, and the reset link can be switched off. Imports only the public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
use laterite_core::{CatalogStore, Db};
use sea_query::{Alias, Expr, Query};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";
const PASSWORD: &str = "Rootpw12345!";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn auth_config(policy: &str) -> AuthConfig {
    serde_json::from_str(&format!(r#"{{"password_policy": {policy}}}"#)).unwrap()
}

fn app(db: &Db, policy: &str) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), auth_config(policy)),
        db.clone(),
        Contributions::default(),
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn session(db: &Db, policy: &str) -> String {
    let svc = AuthService::new(db.clone(), auth_config(policy));
    svc.create_superuser(NewOperator {
        username: "root",
        email: "root@acme.test",
        first_name: "Root",
        last_name: None,
        password: PASSWORD,
        timezone: None,
    })
    .await
    .unwrap();
    let token = svc
        .authenticate("root", PASSWORD, &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
        .await
        .unwrap();
    token
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    session: Option<&str>,
    body: &str,
) -> (StatusCode, String, String) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(session) = session {
        req = req.header("cookie", format!("{SESSION_COOKIE}={session}"));
    }
    if method == "POST" {
        req = req
            .header("content-type", "application/x-www-form-urlencoded")
            .header("sec-fetch-site", "same-origin")
            .header("x-csrf-token", CSRF);
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&bytes).to_string(),
    )
}

const STRICT: &str = r#"{"min_length": 10, "require_uppercase": true, "require_lowercase": true, "require_number": true, "require_nonalpha": true}"#;

#[tokio::test]
async fn preferences_states_the_rules_and_names_each_one_broken() {
    let (db, _g) = test_db().await;
    let app = app(&db, STRICT);
    let root = session(&db, STRICT).await;
    let (status, _, html) = send(&app, "GET", "/admin/preferences", Some(&root), "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(
        "At least 10 characters. An uppercase letter. A lowercase letter. A number. A symbol."
    ));
    let body = format!(
        "current_password={PASSWORD}&new_password=longenoughbutplain&confirm_password=longenoughbutplain"
    );
    let (status, location, _) = send(
        &app,
        "POST",
        "/admin/preferences/password",
        Some(&root),
        &body,
    )
    .await;
    assert!(status.is_redirection());
    assert_eq!(location, "/admin/preferences#password");
    let (_, _, html) = send(&app, "GET", "/admin/preferences", Some(&root), "").await;
    assert!(html.contains("A password needs an uppercase letter."));
    assert!(html.contains("A password needs a number."));
    assert!(html.contains("A password needs a symbol."));
    assert!(!html.contains("A password needs a lowercase letter."));
    // The old password still signs in: nothing changed.
    let svc = AuthService::new(db.clone(), auth_config(STRICT));
    assert!(svc
        .authenticate("root", PASSWORD, &RequestContext::default())
        .await
        .is_ok());
}

#[tokio::test]
async fn an_old_password_opens_nothing_but_the_form_that_replaces_it() {
    let (db, _g) = test_db().await;
    let policy = r#"{"expire_days": 30}"#;
    let app = app(&db, policy);
    let root = session(&db, policy).await;
    // Fresh, the password opens the panel.
    let (status, _, _) = send(&app, "GET", "/admin/users", Some(&root), "").await;
    assert_eq!(status, StatusCode::OK);
    // Forty days on, every page leads to the password form.
    let then = (chrono::Utc::now() - chrono::Duration::days(40))
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Alias::new("backend_users"))
            .value(Alias::new("password_changed_at"), then.clone())
            .value(Alias::new("created_at"), then)
            .and_where(Expr::col(Alias::new("username")).eq("root"))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, location, _) = send(&app, "GET", "/admin/users", Some(&root), "").await;
    assert!(status.is_redirection());
    assert_eq!(location, "/admin/preferences#password");
    let (status, _, html) = send(&app, "GET", "/admin/preferences", Some(&root), "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("This password is more than 30 days old."));
    // Setting a new one opens the panel again.
    let body = format!(
        "current_password={PASSWORD}&new_password=AnotherPw98765!&confirm_password=AnotherPw98765!"
    );
    let (status, _, _) = send(
        &app,
        "POST",
        "/admin/preferences/password",
        Some(&root),
        &body,
    )
    .await;
    assert!(status.is_redirection());
    let (status, _, _) = send(&app, "GET", "/admin/users", Some(&root), "").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_reset_link_can_be_switched_off() {
    let (db, _g) = test_db().await;
    session(&db, "{}").await;
    let open = app(&db, "{}");
    let (status, _, html) = send(&open, "GET", "/admin/login", None, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("/admin/forgot"));
    let (status, _, _) = send(&open, "GET", "/admin/forgot", None, "").await;
    assert_eq!(status, StatusCode::OK);

    let closed = app(&db, r#"{"allow_reset": false}"#);
    let (_, _, html) = send(&closed, "GET", "/admin/login", None, "").await;
    assert!(!html.contains("/admin/forgot"));
    let (status, _, _) = send(&closed, "GET", "/admin/forgot", None, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&closed, "POST", "/admin/forgot", None, "account=root").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
