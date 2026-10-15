//! Components: attached in a page's settings, run before the render, read
//! under their alias; a layout's run too; a bad attachment is refused.
//! Imports only the public API.

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService};
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
