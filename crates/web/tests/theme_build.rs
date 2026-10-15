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
