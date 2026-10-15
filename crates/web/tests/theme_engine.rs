//! The theme engine: layouts, partials, content, escaping, the parent chain,
//! the parse check and the cache. Imports only the public API.

use laterite_core::{Db, Module};
use laterite_web::theme::{self, store, Engine, EngineCache, Kind, ThemeModule};

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

async fn save(db: &Db, theme_id: i64, kind: Kind, path: &str, markup: &str) {
    store::save_template(db, theme_id, kind, path, None, markup, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_page_extends_a_layout_through_the_parent_chain() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Layout,
        "default.htm",
        "<html><title>{{ theme.name }}</title>{% include \"partials/nav.htm\" %}{% block body %}{% endblock %}</html>",
    )
    .await;
    save(
        &db,
        base.id,
        Kind::Partial,
        "nav.htm",
        "<nav>{{ title }}</nav>",
    )
    .await;
    save(
        &db,
        base.id,
        Kind::Content,
        "intro.md",
        "# Hello\n\nA *block*.",
    )
    .await;
    let child = store::create_child_theme(&db, "base", "child", "Child")
        .await
        .unwrap();
    save(
        &db,
        child.id,
        Kind::Page,
        "home.htm",
        "{% extends \"layouts/default.htm\" %}{% block body %}{{ content(\"intro.md\") }}{{ partial(\"nav.htm\", title=\"Acme\") }}{{ user }}{% endblock %}",
    )
    .await;

    let engine = Engine::build(&db, "child").await.unwrap();
    assert_eq!(engine.theme().code, "child");
    assert!(engine.has(Kind::Layout, "default.htm"));
    assert!(engine.has(Kind::Page, "home.htm"));
    assert!(!engine.has(Kind::Page, "about.htm"));
    let html = engine
        .render(
            Kind::Page,
            "home.htm",
            serde_json::json!({"title": "T", "user": "<b>"}),
        )
        .unwrap();
    assert!(
        html.starts_with("<html><title>Child</title><nav>T</nav>"),
        "{html}"
    );
    assert!(html.contains("<h1>Hello</h1>"), "{html}");
    assert!(html.contains("A <em>block</em>."), "{html}");
    assert!(html.contains("<nav>Acme</nav>"), "{html}");
    assert!(html.contains("&lt;b&gt;"), "{html}");
}

#[tokio::test]
async fn assets_are_not_escaped_and_content_kinds_differ() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Asset,
        "css/site.css",
        "a > b { color: {{ colour }} }",
    )
    .await;
    save(&db, base.id, Kind::Content, "raw.htm", "<b>raw</b>").await;
    save(&db, base.id, Kind::Content, "plain.txt", "<b>text</b>").await;
    save(
        &db,
        base.id,
        Kind::Page,
        "p.htm",
        "{{ content(\"raw.htm\") }}|{{ content(\"plain.txt\") }}",
    )
    .await;
    let engine = Engine::build(&db, "base").await.unwrap();
    let css = engine
        .render(
            Kind::Asset,
            "css/site.css",
            serde_json::json!({"colour": "red"}),
        )
        .unwrap();
    assert_eq!(css, "a > b { color: red }");
    let page = engine.render(Kind::Page, "p.htm", ()).unwrap();
    assert_eq!(page, "<b>raw</b>|&lt;b&gt;text&lt;&#x2f;b&gt;");
    let err = engine.render(Kind::Page, "missing.htm", ()).unwrap_err();
    assert_eq!(err.kind, "template not found");
}

#[tokio::test]
async fn a_printed_undefined_is_an_error_and_a_tested_one_is_not() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Page,
        "tested.htm",
        "{% if page.sub %}x{% else %}y{% endif %}",
    )
    .await;
    save(&db, base.id, Kind::Page, "printed.htm", "{{ page.sub }}").await;
    let engine = Engine::build(&db, "base").await.unwrap();
    assert_eq!(
        engine
            .render(Kind::Page, "tested.htm", serde_json::json!({"page": {}}))
            .unwrap(),
        "y"
    );
    let err = engine
        .render(Kind::Page, "printed.htm", serde_json::json!({"page": {}}))
        .unwrap_err();
    assert_eq!(err.template.as_deref(), Some("pages/printed.htm"));
    assert_eq!(err.line, Some(1));
    assert_eq!(err.kind, "undefined value");
}

#[tokio::test]
async fn a_runaway_loop_runs_out_of_fuel() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Page,
        "loop.htm",
        "{% for i in range(10000) %}{% for j in range(10000) %}{{ j }}{% endfor %}{% endfor %}",
    )
    .await;
    let engine = Engine::build(&db, "base").await.unwrap();
    let err = engine.render(Kind::Page, "loop.htm", ()).unwrap_err();
    assert_eq!(err.kind, "engine ran out of fuel");
}

#[test]
fn the_check_points_at_the_line() {
    assert!(theme::check("ok {{ a }}\n{% if b %}x{% endif %}").is_ok());
    let err = theme::check("line one\n{% if b %}x\n{{ unclosed ").unwrap_err();
    assert_eq!(err.kind, "syntax error");
    assert_eq!(err.line, Some(3), "{err}");
}

#[tokio::test]
async fn the_cache_rebuilds_when_the_theme_or_an_ancestor_changes() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(&db, base.id, Kind::Partial, "nav.htm", "one").await;
    let child = store::create_child_theme(&db, "base", "child", "Child")
        .await
        .unwrap();
    save(
        &db,
        child.id,
        Kind::Page,
        "p.htm",
        "{% include \"partials/nav.htm\" %}",
    )
    .await;
    let cache = EngineCache::new();
    let first = cache.get(&db, "child").await.unwrap();
    assert_eq!(first.render(Kind::Page, "p.htm", ()).unwrap(), "one");
    let again = cache.get(&db, "child").await.unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &again));

    // A later stamp: the clock may not have moved, so push it by hand.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    save(&db, base.id, Kind::Partial, "nav.htm", "two").await;
    let rebuilt = cache.get(&db, "child").await.unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &rebuilt));
    assert_eq!(rebuilt.render(Kind::Page, "p.htm", ()).unwrap(), "two");

    cache.invalidate("child");
    let fresh = cache.get(&db, "child").await.unwrap();
    assert!(!std::sync::Arc::ptr_eq(&rebuilt, &fresh));
}
