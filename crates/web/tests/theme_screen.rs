//! The Themes screen: gated by its permission, creating a theme and a
//! template, refusing a template that does not parse. Imports only the
//! public API.

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{
    password, store as auth_store, AuthConfig, AuthService, NewOperator, RequestContext,
};
use laterite_core::{CatalogStore, Db, Module};
use laterite_web::theme::{self, store, Kind, ThemeModule};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    let mut contributions = Contributions::default();
    contributions.permissions.push(theme::admin::permission());
    contributions.screens.push(theme::admin::screen());
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        contributions,
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// A superuser's session and a plain operator's, the first with a known
/// request token.
async fn sessions(db: &Db) -> (String, String) {
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
    let hash = password::hash_password("plainpw12345").unwrap();
    auth_store::create_user(db, "plain", "plain@acme.test", "Plain", None, &hash, false)
        .await
        .unwrap();
    let mut tokens = Vec::new();
    for (user, pw) in [("root", "rootpw12345"), ("plain", "plainpw12345")] {
        tokens.push(
            svc.authenticate(user, pw, &RequestContext::default())
                .await
                .unwrap()
                .token,
        );
    }
    svc.set_session_data(&tokens[0], r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    (tokens.remove(0), tokens.remove(0))
}

async fn get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = laterite_admin::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Posts a form as the root session and returns the status and the redirect.
async fn post(app: &Router, path: &str, session: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, location)
}

#[tokio::test]
async fn the_screen_is_gated_and_lists_themes() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, plain) = sessions(&db).await;
    store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    theme::set_active_theme(&db, "base").await.unwrap();

    let (status, html) = get(&app, "/admin/themes", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Base") && html.contains(">base<"), "{html}");
    assert!(html.contains("Active"), "{html}");
    assert!(html.contains("New theme"), "{html}");

    let (status, _) = get(&app, "/admin/themes", &plain).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, html) = get(&app, "/admin", &plain).await;
    assert!(!html.contains("/admin/themes"), "out of their menu: {html}");
    let (_, html) = get(&app, "/admin", &root).await;
    assert!(html.contains("/admin/themes"), "in the menu: {html}");
}

#[tokio::test]
async fn a_theme_and_a_template_are_created_and_a_broken_template_is_refused() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;

    let (status, location) = post(&app, "/admin/themes", &root, "code=acme&name=Acme+Site").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/admin/themes/acme");
    let created = store::find_theme(&db, "acme").await.unwrap().unwrap();
    assert_eq!(created.config.name, "Acme Site");

    let (status, html) = get(&app, "/admin/themes/acme", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Theme created."), "{html}");
    assert!(
        html.contains("Layouts") && html.contains("theme.yaml"),
        "{html}"
    );

    let (status, location) = post(
        &app,
        "/admin/themes/acme/templates",
        &root,
        "kind=page&path=home.htm",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let t = store::find_template(&db, created.id, Kind::Page, "home.htm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(location, format!("/admin/themes/acme/templates/{}", t.id));

    let editor = format!("/admin/themes/acme/templates/{}", t.id);
    let (status, _) = post(
        &app,
        &format!("{editor}/save"),
        &root,
        "settings=%7B%22url%22%3A%22%2F%22%7D&markup=%7B%25+if+%25%7D",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = get(&app, &editor, &root).await;
    assert!(html.contains("syntax error"), "{html}");
    assert_eq!(
        store::template(&db, t.id).await.unwrap().unwrap().markup,
        "",
        "a broken template is not saved"
    );

    let (status, _) = post(
        &app,
        &format!("{editor}/save"),
        &root,
        "settings=%7B%22url%22%3A%22%2F%22%7D&markup=%3Ch1%3E%7B%7B+theme.name+%7D%7D%3C%2Fh1%3E",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = store::template(&db, t.id).await.unwrap().unwrap();
    assert_eq!(saved.markup, "<h1>{{ theme.name }}</h1>");
    assert_eq!(saved.settings, serde_json::json!({"url": "/"}));
    let (_, html) = get(&app, &editor, &root).await;
    assert!(
        html.contains("Template saved.") && html.contains("Restore"),
        "{html}"
    );

    // A template is reached only under its own theme.
    store::create_theme(&db, "other", "name: Other\n")
        .await
        .unwrap();
    let (status, location) = get(
        &app,
        &format!("/admin/themes/other/templates/{}", t.id),
        &root,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{location}");

    let (status, location) = post(&app, "/admin/themes/other/delete", &root, "").await;
    assert_eq!(
        (status, location.as_str()),
        (StatusCode::SEE_OTHER, "/admin/themes")
    );
    assert!(store::find_theme(&db, "other").await.unwrap().is_none());
}
