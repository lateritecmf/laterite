//! A theme built to files: fixed pages at their URLs, the 404 page as
//! 404.html, assets under the theme's path, parameterised and hidden pages
//! skipped. Imports only the public API.

use laterite_core::{Db, Module};
use laterite_web::theme::{build, store, Kind, ThemeModule};

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

#[tokio::test]
async fn a_theme_builds_its_fixed_pages_and_assets() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    let save =
        |kind, path: &'static str, settings: Option<serde_json::Value>, markup: &'static str| {
            let db = db.clone();
            async move {
                store::save_template(&db, base.id, kind, path, settings, markup, None)
                    .await
                    .unwrap();
            }
        };
    save(
        Kind::Layout,
        "default.htm",
        None,
        "<head>{{ head }}</head>{% block page %}{% endblock %}",
    )
    .await;
    save(
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/", "layout": "default", "title": "Home"})),
        "<a href=\"{{ url('/about') }}\">{{ asset('css/site.css') }}</a>",
    )
    .await;
    save(
        Kind::Page,
        "about.htm",
        Some(serde_json::json!({"url": "/about"})),
        "about",
    )
    .await;
    save(
        Kind::Page,
        "post.htm",
        Some(serde_json::json!({"url": "/blog/:slug"})),
        "post",
    )
    .await;
    save(
        Kind::Page,
        "secret.htm",
        Some(serde_json::json!({"url": "/secret", "hidden": true})),
        "x",
    )
    .await;
    save(
        Kind::Page,
        "404.htm",
        Some(serde_json::json!({"url": "/404"})),
        "lost",
    )
    .await;
    save(Kind::Asset, "css/site.css", None, "a{}").await;

    let out = tempfile::tempdir().unwrap();
    let report = build::build(&db, "base", out.path(), "https://acme.test/")
        .await
        .unwrap();
    assert_eq!(report.pages, ["/", "/404", "/about"]);
    assert_eq!(report.assets, ["themes/base/assets/css/site.css"]);
    assert_eq!(report.skipped, ["post", "secret"]);

    let home = std::fs::read_to_string(out.path().join("index.html")).unwrap();
    assert!(home.contains("<title>Home</title>"), "{home}");
    assert!(
        home.contains("<link rel=\"canonical\" href=\"https://acme.test/\">"),
        "{home}"
    );
    assert!(
        home.contains("href=\"https://acme.test/about\">/themes/base/assets/css/site.css</a>"),
        "{home}"
    );
    assert_eq!(
        std::fs::read_to_string(out.path().join("about/index.html")).unwrap(),
        "about"
    );
    assert_eq!(
        std::fs::read_to_string(out.path().join("404.html")).unwrap(),
        "lost"
    );
    assert_eq!(
        std::fs::read_to_string(out.path().join("themes/base/assets/css/site.css")).unwrap(),
        "a{}"
    );
    assert!(!out.path().join("secret").exists());
}

/// A page with parameters is built once per set a module answers for its
/// pattern, each at the URL the parameters fill.
#[tokio::test]
async fn a_page_with_parameters_is_built_for_every_set_a_module_answers() {
    use laterite_web::theme::build::{Paths, PathsReg};
    use laterite_web::theme::router::Params;
    use laterite_web::theme::ThemeError;

    struct Slugs;
    #[laterite_core::strata::async_trait]
    impl Paths for Slugs {
        async fn params(&self, _db: &Db) -> Result<Vec<Params>, ThemeError> {
            Ok(["first", "second"]
                .into_iter()
                .map(|slug| {
                    let mut p = Params::new();
                    p.insert("slug".into(), slug.into());
                    p
                })
                .collect())
        }
    }

    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "post.htm",
        Some(serde_json::json!({"url": "/blog/:slug"})),
        "post {{ this.params.slug }} at {{ this.url }}",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "orphan.htm",
        Some(serde_json::json!({"url": "/orphan/:id"})),
        "x",
        None,
    )
    .await
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    let reg = PathsReg::new("/blog/:slug", std::sync::Arc::new(Slugs));
    let report = build::build_with(
        &db,
        "base",
        out.path(),
        "https://acme.test",
        &[],
        &[],
        &[&reg],
    )
    .await
    .unwrap();
    assert_eq!(report.pages, vec!["/blog/first", "/blog/second"]);
    assert_eq!(report.skipped, vec!["orphan"]);
    let first = std::fs::read_to_string(out.path().join("blog/first/index.html")).unwrap();
    assert!(first.starts_with("post first at "), "{first}");
    assert!(first.contains("blog&#x2f;first&#x2f;"), "{first}");
    assert!(out.path().join("blog/second/index.html").is_file());
}

/// A page attaching a component with handlers builds, with a warning: a
/// static host answers no post.
#[tokio::test]
async fn a_page_with_a_handler_component_builds_with_a_warning() {
    use laterite_web::theme::component::{
        Component, ComponentContext, ComponentOutput, ComponentReg, Handled, HandlerContext,
    };
    use laterite_web::theme::ThemeError;

    struct Form;
    #[laterite_core::strata::async_trait]
    impl Component for Form {
        fn markup(&self) -> Option<&'static str> {
            Some("<form></form>")
        }
        async fn run(&self, _cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError> {
            Ok(ComponentOutput::data(serde_json::json!({})))
        }
        fn handlers(&self) -> &'static [&'static str] {
            &["send"]
        }
        async fn handle(&self, _n: &str, _cx: &HandlerContext<'_>) -> Result<Handled, ThemeError> {
            Ok(Handled::Unknown)
        }
    }

    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "contact.htm",
        Some(serde_json::json!({
            "url": "/contact",
            "components": {"contact": {"component": "acme.form"}}
        })),
        "{{ component('contact') }}",
        None,
    )
    .await
    .unwrap();
    let reg = ComponentReg::new("acme.form", "Form", std::sync::Arc::new(Form));
    let out = tempfile::tempdir().unwrap();
    let report = build::build_with(
        &db,
        "base",
        out.path(),
        "https://acme.test",
        &[],
        &[&reg],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(report.pages, vec!["/contact"]);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("contact") && report.warnings[0].contains("acme.form"),
        "{:?}",
        report.warnings
    );
}
