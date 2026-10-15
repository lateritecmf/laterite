//! The public site: pages served from the active theme, preview for an
//! operator, the theme's 404 and error pages, and assets. Imports only the
//! public API.

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{
    password, store as auth_store, AuthConfig, AuthService, NewOperator, RequestContext,
};
use laterite_core::{CatalogStore, Db, Module};
use laterite_web::theme::{self, site, store, Kind, ThemeModule};
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
    contributions.public_routes.push(site::route());
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        contributions,
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn get(
    app: &Router,
    path: &str,
    session: Option<&str>,
) -> (StatusCode, String, Vec<(String, String)>) {
    let mut req = Request::builder().uri(path);
    if let Some(s) = session {
        req = req.header("cookie", format!("{SESSION_COOKIE}={s}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let bytes = laterite_admin::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string(), headers)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

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
    (tokens.remove(0), tokens.remove(0))
}

async fn site_theme(db: &Db) -> theme::Theme {
    let base = store::create_theme(db, "base", "name: Base\n")
        .await
        .unwrap();
    let save =
        |kind, path: &'static str, settings: Option<serde_json::Value>, markup: &'static str| {
            let db = db.clone();
            let id = base.id;
            async move {
                store::save_template(&db, id, kind, path, settings, markup, None)
                    .await
                    .unwrap();
            }
        };
    save(
        Kind::Layout,
        "default.htm",
        None,
        "<html><head>{{ head }}</head><body>{% block page %}{% endblock %}</body></html>",
    )
    .await;
    save(
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/", "layout": "default", "title": "Home of {{ this.theme.name }}", "description": "The site"})),
        "<h1>{{ this.page.title }}</h1><a href=\"{{ page('blog/post', slug='x') }}\">post</a>{% if this.preview %}<i>preview</i>{% endif %}",
    )
    .await;
    save(
        Kind::Page,
        "blog/post.htm",
        Some(serde_json::json!({"url": "/blog/:slug", "layout": "default"})),
        "<p>{{ this.params.slug }}</p>",
    )
    .await;
    save(
        Kind::Page,
        "secret.htm",
        Some(serde_json::json!({"url": "/secret", "hidden": true})),
        "secret",
    )
    .await;
    save(
        Kind::Page,
        "404.htm",
        Some(serde_json::json!({"url": "/404"})),
        "lost at {{ this.url }}",
    )
    .await;
    save(
        Kind::Page,
        "broken.htm",
        Some(serde_json::json!({"url": "/broken"})),
        "{{ nothing.here }}",
    )
    .await;
    save(
        Kind::Page,
        "error.htm",
        Some(serde_json::json!({"url": "/error"})),
        "sorry",
    )
    .await;
    save(
        Kind::Asset,
        "css/site.css",
        None,
        "body{color:{{ theme.name|lower }}}",
    )
    .await;
    theme::set_active_theme(db, "base").await.unwrap();
    base
}

#[tokio::test]
async fn pages_are_served_from_the_active_theme() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    site_theme(&db).await;

    let (status, html, headers) = get(&app, "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<title>Home of Base</title>"), "{html}");
    assert!(
        html.contains("<meta name=\"description\" content=\"The site\">"),
        "{html}"
    );
    assert!(
        html.contains("<link rel=\"canonical\" href=\"https://acme.test/\">"),
        "{html}"
    );
    assert!(
        html.contains("<h1>Home of {{ this.theme.name }}</h1>")
            || html.contains("<h1>Home of Base</h1>"),
        "{html}"
    );
    assert!(html.contains("href=\"/blog/x\""), "{html}");
    assert!(!html.contains("<i>preview</i>"), "{html}");
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(header(&headers, "cache-control"), Some("public, no-cache"));

    let (status, html, _) = get(&app, "/blog/hello", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<p>hello</p>"), "{html}");

    let (status, html, _) = get(&app, "/nowhere", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(html, "lost at &#x2f;nowhere", "the theme's own 404 page");

    let (status, html, _) = get(&app, "/broken", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(html, "sorry", "the theme's own error page");

    let (status, html, _) = get(&app, "/admin/nowhere", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        html.contains("<!DOCTYPE html>"),
        "the admin keeps its styled 404: {html}"
    );
}

#[tokio::test]
async fn an_operator_previews_hidden_pages_and_their_edit_theme() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    site_theme(&db).await;
    let (root, plain) = sessions(&db).await;

    let (status, _, _) = get(&app, "/secret", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(&app, "/secret", Some(&plain)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "no themes permission, no preview"
    );
    let (status, html, headers) = get(&app, "/secret", Some(&root)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(html, "secret");
    assert_eq!(header(&headers, "cache-control"), Some("private, no-store"));
    let (_, html, _) = get(&app, "/", Some(&root)).await;
    assert!(html.contains("<i>preview</i>"), "{html}");

    // The edit theme shows for its operator alone.
    let next = store::duplicate_theme(&db, "base", "next").await.unwrap();
    store::save_template(
        &db,
        next.id,
        Kind::Page,
        "home.htm",
        None,
        "next home",
        None,
    )
    .await
    .unwrap();
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let root_id = svc.verify_session(&root).await.unwrap().user.id;
    theme::set_edit_theme(&db, root_id, Some("next"))
        .await
        .unwrap();
    let (_, html, _) = get(&app, "/", Some(&root)).await;
    assert!(html.contains("next home"), "{html}");
    let (_, html, _) = get(&app, "/", None).await;
    assert!(!html.contains("next home"), "{html}");
}

#[tokio::test]
async fn assets_serve_with_a_validator() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    site_theme(&db).await;
    let (status, body, headers) = get(&app, "/themes/base/assets/css/site.css", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "body{color:base}");
    assert_eq!(
        header(&headers, "content-type"),
        Some("text/css; charset=utf-8")
    );
    let etag = header(&headers, "etag").unwrap().to_string();
    let req = Request::builder()
        .uri("/themes/base/assets/css/site.css")
        .header("if-none-match", &etag)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    let (status, _, _) = get(&app, "/themes/base/assets/css/none.css", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
