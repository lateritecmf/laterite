//! Components: attached in a page's settings, run before the render, read
//! under their alias; a layout's run too; a bad attachment is refused.
//! Imports only the public API.

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db, Module};
use laterite_web::theme::component::{
    self, Component, ComponentContext, ComponentOutput, ComponentReg, PropertyDef, PropertyKind,
};
use laterite_web::theme::{self, site, store, Kind, ThemeError, ThemeModule};
use std::sync::Arc;
use tower::ServiceExt;

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

/// `acme.posts`: the latest `limit` posts, and the one named by `slug`.
struct Posts;

#[laterite_core::strata::async_trait]
impl Component for Posts {
    fn markup(&self) -> Option<&'static str> {
        Some("<ul>{% for i in component.items %}<li>{{ i }}</li>{% endfor %}</ul>")
    }
    fn properties(&self) -> Vec<PropertyDef> {
        vec![
            PropertyDef::new("limit", "How many", PropertyKind::Number).default(3),
            PropertyDef::new("slug", "Slug", PropertyKind::Text),
        ]
    }
    async fn run(&self, cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError> {
        let limit = cx.properties["limit"].as_u64().unwrap_or(0);
        let items: Vec<String> = (1..=limit).map(|i| format!("post-{i}")).collect();
        let slug = cx
            .properties
            .get("slug")
            .and_then(|s| s.as_str())
            .unwrap_or("none");
        Ok(ComponentOutput::data(
            serde_json::json!({"items": items, "slug": slug, "path": cx.path}),
        )
        .inject("site_name", format!("Site of {}", cx.theme)))
    }
}

fn regs() -> Vec<ComponentReg> {
    vec![ComponentReg::new("acme.posts", "Posts", Arc::new(Posts))]
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    let mut contributions = Contributions::default();
    contributions.permissions.push(theme::admin::permission());
    contributions.screens.push(theme::admin::screen());
    contributions.public_routes.push(site::route());
    let mut registry = laterite_core::Registry::new();
    registry.set_owner(laterite_core::ModuleId::new("acme.blog"));
    for reg in regs() {
        registry.add(reg);
    }
    contributions.plugin_defined = Arc::new(registry);
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        contributions,
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn get(app: &Router, path: &str) -> (StatusCode, String) {
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = laterite_admin::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn attached_components_run_and_read_under_their_alias() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Layout,
        "default.htm",
        Some(
            serde_json::json!({"components": {"recent": {"component": "acme.posts", "limit": 2}}}),
        ),
        "[{{ recent.items|join(',') }}][{{ site_name }}]{% block page %}{% endblock %}",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "post.htm",
        Some(serde_json::json!({
            "url": "/blog/:slug",
            "layout": "default",
            "components": {"post": {"component": "acme.posts", "limit": 1, "slug": "{{ :slug }}"}}
        })),
        "<p>{{ post.slug }}/{{ post.items|length }}/{{ post.path }}</p>",
        None,
    )
    .await
    .unwrap();
    theme::set_active_theme(&db, "base").await.unwrap();

    let (status, html) = get(&app, "/blog/hello").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        html,
        "[post-1,post-2][Site of base]<p>hello/1/&#x2f;blog&#x2f;hello</p>"
    );

    // The build runs them too.
    let out = tempfile::tempdir().unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/", "layout": "default"})),
        "home",
        None,
    )
    .await
    .unwrap();
    let list = regs();
    let refs: Vec<&ComponentReg> = list.iter().collect();
    theme::build::build_with(&db, "base", out.path(), "https://acme.test", &[], &refs)
        .await
        .unwrap();
    let home = std::fs::read_to_string(out.path().join("index.html")).unwrap();
    assert_eq!(home, "[post-1,post-2][Site of base]home");
}

#[test]
fn a_bad_attachment_is_refused_by_the_check() {
    let list = regs();
    let refs: Vec<&ComponentReg> = list.iter().collect();
    let ok = serde_json::json!({"components": {"a": {"component": "acme.posts", "limit": 2, "slug": "{{ :slug }}"}}});
    assert!(component::check(&ok, &refs).is_ok());
    let unknown = serde_json::json!({"components": {"a": {"component": "acme.none"}}});
    assert!(matches!(
        component::check(&unknown, &refs),
        Err(ThemeError::Page(_))
    ));
    let extra =
        serde_json::json!({"components": {"a": {"component": "acme.posts", "colour": "red"}}});
    assert!(component::check(&extra, &refs).is_err());
    let wrong =
        serde_json::json!({"components": {"a": {"component": "acme.posts", "limit": "two"}}});
    assert!(component::check(&wrong, &refs).is_err());
}

#[tokio::test]
async fn component_markup_falls_back_to_the_default_and_a_theme_overrides_it() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({
            "url": "/",
            "components": {"recent": {"component": "acme.posts", "limit": 2}}
        })),
        "{{ component('recent') }}|{{ component('nothing') }}",
        None,
    )
    .await
    .unwrap();
    theme::set_active_theme(&db, "base").await.unwrap();
    let (status, _) = get(&app, "/").await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unattached alias is an error"
    );

    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "home.htm",
        None,
        "{{ component('recent') }}",
        None,
    )
    .await
    .unwrap();
    let (status, html) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        html, "<ul><li>post-1</li><li>post-2</li></ul>",
        "the default markup"
    );

    store::save_template(
        &db,
        base.id,
        Kind::Partial,
        "recent/default.htm",
        None,
        "<ol>{{ recent.items|length }}:{{ component.items[0] }}</ol>",
        None,
    )
    .await
    .unwrap();
    let (_, html) = get(&app, "/").await;
    assert_eq!(html, "<ol>2:post-1</ol>", "the theme's own partial wins");
}

#[tokio::test]
async fn the_editor_takes_components_as_json_and_checks_them() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let theme = store::create_theme(&db, "acme", "name: Acme\n")
        .await
        .unwrap();
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
    let root = svc
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&root, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    let t = store::save_template(
        &db,
        theme.id,
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/"})),
        "x",
        None,
    )
    .await
    .unwrap();
    let editor = format!("/admin/themes/acme/templates/{}", t.id);
    let post = |body: &'static str| {
        let app = app.clone();
        let root = root.clone();
        let editor = editor.clone();
        async move {
            let req = Request::builder()
                .method("POST")
                .uri(format!("{editor}/save"))
                .header("cookie", format!("laterite_session={root}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", "tok-123")
                .body(Body::from(body))
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        }
    };
    let page = |path: String| {
        let app = app.clone();
        let root = root.clone();
        async move {
            let req = Request::builder()
                .uri(path)
                .header("cookie", format!("laterite_session={root}"))
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(req).await.unwrap();
            let bytes = laterite_admin::axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8_lossy(&bytes).to_string()
        }
    };

    let html = page(editor.clone()).await;
    assert!(
        html.contains(r#"name="components""#) && html.contains("acme.posts (limit, slug)"),
        "{html}"
    );

    // A component the registry lacks is refused; a declared one is kept.
    post("url=%2F&markup=x&components=%7B%22a%22%3A%7B%22component%22%3A%22acme.none%22%7D%7D")
        .await;
    let html = page(editor.clone()).await;
    assert!(html.contains("no component"), "{html}");
    post("url=%2F&markup=x&components=%7B%22recent%22%3A%7B%22component%22%3A%22acme.posts%22%2C%22limit%22%3A2%7D%7D").await;
    let saved = store::template(&db, t.id).await.unwrap().unwrap();
    assert_eq!(saved.settings["components"]["recent"]["limit"], 2);
    let html = page(editor.clone()).await;
    let box_at = html.find(r#"name="components""#).expect("the box");
    let after = &html[box_at..(box_at + 600).min(html.len())];
    assert!(
        after.contains("recent") && after.contains("acme.posts"),
        "the box shows the JSON: {after}"
    );
    post("url=%2F&markup=x&components=").await;
    let saved = store::template(&db, t.id).await.unwrap().unwrap();
    assert_eq!(saved.settings["components"], serde_json::json!({}));
}
