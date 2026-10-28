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
    assert!(html.contains(r##"<h1 id="hello">Hello</h1>"##), "{html}");
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

#[tokio::test]
async fn a_template_reads_the_settings_beside_the_theme_keys() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(
        &db,
        "base",
        "name: Base\nform:\n  fields:\n    site_name:\n      label: Site name\n      type: text\n",
    )
    .await
    .unwrap();
    store::save_settings(&db, "base", &serde_json::json!({"site_name": "Acme <Co>"}))
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Page,
        "p.htm",
        "{{ theme.site_name }}|{{ theme.name }}",
    )
    .await;
    let engine = Engine::build(&db, "base").await.unwrap();
    assert_eq!(
        engine.render(Kind::Page, "p.htm", ()).unwrap(),
        "Acme &lt;Co&gt;|Base"
    );
}

#[tokio::test]
async fn a_page_renders_inside_its_layout_and_routes_resolve() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Layout,
        "default.htm",
        "<title>{% block title %}Base{% endblock %}</title><main>{% block page %}{% endblock %}</main>",
    )
    .await;
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/", "layout": "default", "title": "Home"})),
        "{% block title %}Home{% endblock %}<h1>{{ this.page.title }}</h1><a href=\"{{ page('blog/post', slug='hi') }}\">x</a>",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "blog/post.htm",
        Some(serde_json::json!({"url": "/blog/:slug", "layout": "default"})),
        "<p>{{ this.params.slug }}</p>",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "bare.htm",
        Some(serde_json::json!({"url": "/bare"})),
        "bare {{ x }}",
        None,
    )
    .await
    .unwrap();
    let child = store::create_child_theme(&db, "base", "child", "Child")
        .await
        .unwrap();
    store::save_template(
        &db,
        child.id,
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/", "layout": "default"})),
        "{% extends \"layouts/default.htm\" %}{% block page %}child home{% endblock %}",
        None,
    )
    .await
    .unwrap();

    let engine = Engine::build(&db, "base").await.unwrap();
    assert_eq!(engine.routes().names(), ["bare", "blog/post", "home"]);
    let (name, params) = engine.resolve("/blog/hello").unwrap();
    assert_eq!(name, "blog/post");
    assert_eq!(params.get("slug").map(String::as_str), Some("hello"));
    assert!(engine.resolve("/nowhere").is_none());
    assert_eq!(engine.page("home").unwrap().title, "Home");
    assert_eq!(
        engine
            .url_for(
                "blog/post",
                &[("slug".to_string(), "a-b".to_string())].into()
            )
            .as_deref(),
        Some("/blog/a-b")
    );

    let html = engine
        .render(
            Kind::Page,
            "home.htm",
            serde_json::json!({"this": {"page": {"title": "Home"}, "params": {}}}),
        )
        .unwrap();
    assert_eq!(
        html, "<title>Home</title><main><h1>Home</h1><a href=\"/blog/hi\">x</a></main>",
        "the page fills the layout's page block and overrides its title block"
    );
    let html = engine
        .render(Kind::Page, "bare.htm", serde_json::json!({"x": 1}))
        .unwrap();
    assert_eq!(html, "bare 1", "no layout renders the page alone");

    // A page that extends on its own is left as written.
    let engine = Engine::build(&db, "child").await.unwrap();
    let html = engine.render(Kind::Page, "home.htm", ()).unwrap();
    assert_eq!(html, "<title>Base</title><main>child home</main>");

    // An error in a page keeps the author's line number.
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "broken.htm",
        Some(serde_json::json!({"url": "/broken", "layout": "default"})),
        "line one\nline two {{ missing }}",
        None,
    )
    .await
    .unwrap();
    let engine = Engine::build(&db, "base").await.unwrap();
    let err = engine.render(Kind::Page, "broken.htm", ()).unwrap_err();
    assert_eq!(err.line, Some(2), "{err}");
}

#[tokio::test]
async fn partials_carry_this_and_content_takes_variables() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Partial,
        "card.htm",
        "<b>{{ title }}</b>{{ this.page.title }}{% if head is defined %}+head{% endif %}",
    )
    .await;
    save(
        &db,
        base.id,
        Kind::Content,
        "hello.md",
        "Hello **{name}** from {where}",
    )
    .await;
    save(&db, base.id, Kind::Asset, "css/site.css", "a{}").await;
    save(
        &db,
        base.id,
        Kind::Page,
        "p.htm",
        "{{ partial(\"card.htm\", title=\"T\") }}|{{ content(\"hello.md\", name=\"<Ann>\", where=\"here\") }}|{{ asset(\"css/site.css\") }}",
    )
    .await;
    let engine = Engine::build(&db, "base").await.unwrap();
    let html = engine
        .render(
            Kind::Page,
            "p.htm",
            serde_json::json!({"this": {"page": {"title": "P"}}, "head": "<title>x</title>"}),
        )
        .unwrap();
    assert_eq!(
        html,
        "<b>T</b>P+head|<p>Hello <strong>&lt;Ann&gt;</strong> from here</p>\n|/themes/base/assets/css/site.css"
    );
    let html = engine
        .render(
            Kind::Page,
            "p.htm",
            serde_json::json!({"this": {"page": {"title": "P"}}}),
        )
        .unwrap();
    assert!(html.starts_with("<b>T</b>P|"), "{html}");
}

/// `media()` takes a resolved value or a URL and `|variant` names a preset
/// in it; a value with nothing in it renders empty.
#[tokio::test]
async fn media_helpers_build_file_and_variant_urls() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    save(
        &db,
        base.id,
        Kind::Page,
        "p.htm",
        "{{ media(cover) }}|{{ media(cover, preset=\"card\") }}|{{ cover.url|variant(\"thumb\") }}|{{ media(cover.url|variant(\"card\"), preset=\"hero\") }}|{{ media(none) }}|{{ media(\"\") }}|{{ \"/other/x.png\"|variant(\"card\") }}",
    )
    .await;
    let engine = Engine::build(&db, "base").await.unwrap();
    let hash = "a".repeat(64);
    let cover =
        serde_json::json!({ "url": format!("/storage/media/{hash}/cat.png"), "name": "cat.png" });
    let html = engine
        .render(
            Kind::Page,
            "p.htm",
            serde_json::json!({ "cover": cover, "none": null }),
        )
        .unwrap();
    assert_eq!(
        html,
        format!(
            "/storage/media/{hash}/cat.png|/storage/media/{hash}/card/cat.png|/storage/media/{hash}/thumb/cat.png|/storage/media/{hash}/hero/cat.png|||/other/x.png"
        )
    );
    assert_eq!(
        theme::variant(
            format!("https://acme.test/storage/media/{hash}/thumb/cat.png"),
            "card".into()
        ),
        format!("https://acme.test/storage/media/{hash}/card/cat.png")
    );
}
