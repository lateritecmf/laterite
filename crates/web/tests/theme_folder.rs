//! A theme folder on disk builds to a site with no application: loaded into
//! a private in-memory database and rendered through the usual build.
//! Imports only the public API.

use laterite_web::theme::{build, folder};
use std::fs;

fn theme_folder(root: &std::path::Path) -> std::path::PathBuf {
    let dir = root.join("acme");
    for sub in ["layouts", "pages", "partials", "content", "assets/css"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("theme.yaml"), "name: Acme\nauthor: Acme\n").unwrap();
    fs::write(
        dir.join("layouts/default.htm"),
        "<!doctype html><html><head>{{ head }}<link rel=\"stylesheet\" href=\"{{ asset('css/site.css') }}\"></head><body>{% include \"partials/nav.htm\" %}<main>{% block page %}{% endblock %}</main></body></html>",
    )
    .unwrap();
    fs::write(
        dir.join("pages/home.htm"),
        "url: /\nlayout: default\ntitle: Acme\n==\n<h1>{{ this.page.title }}</h1>{{ content(\"intro.md\") }}",
    )
    .unwrap();
    fs::write(
        dir.join("pages/404.htm"),
        "url: /404\nlayout: default\ntitle: Missing\n==\n<h1>Missing</h1>",
    )
    .unwrap();
    fs::write(
        dir.join("pages/post.htm"),
        "url: /post/:slug\nlayout: default\n==\n<h1>{{ this.params.slug }}</h1>",
    )
    .unwrap();
    fs::write(
        dir.join("pages/robots.htm"),
        "url: /robots.txt\n==\nUser-agent: *\nAllow: /\nSitemap: {{ url('/sitemap.xml') }}\n",
    )
    .unwrap();
    fs::write(
        dir.join("partials/nav.htm"),
        "<nav><a href=\"{{ url('/') }}\">Home</a></nav>",
    )
    .unwrap();
    fs::write(dir.join("content/intro.md"), "## Welcome\n\nHello.\n").unwrap();
    fs::write(dir.join("assets/css/site.css"), "body { margin: 0 }").unwrap();
    // Hidden entries are left alone.
    fs::write(dir.join(".DS_Store"), "junk").unwrap();
    dir
}

#[tokio::test]
async fn a_theme_folder_builds_to_a_site() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = theme_folder(tmp.path());
    let loaded = folder::load(&dir).await.unwrap();
    assert_eq!(loaded.code, "acme");
    assert!(loaded.report.created);
    assert_eq!(loaded.report.imported, 8);
    assert!(
        loaded.report.ignored.is_empty(),
        "{:?}",
        loaded.report.ignored
    );

    let out = tmp.path().join("dist");
    let report = build::build(&loaded.db, &loaded.code, &out, "https://acme.example")
        .await
        .unwrap();
    assert_eq!(
        report.pages,
        vec![
            "/".to_string(),
            "/404".to_string(),
            "/robots.txt".to_string()
        ]
    );
    assert_eq!(report.skipped, vec!["post".to_string()]);
    let home = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(home.contains("<h1>Acme</h1>"), "{home}");
    assert!(
        home.contains(r##"<h2 id="welcome">Welcome</h2>"##),
        "{home}"
    );
    assert!(home.contains("https://acme.example/"), "{home}");
    assert!(home.contains("/themes/acme/assets/css/site.css"), "{home}");
    let about = fs::read_to_string(out.join("404/index.html")).unwrap();
    assert!(
        about.contains(r#"<link rel="canonical" href="https://acme.example/404/">"#),
        "{about}"
    );
    assert!(out.join("404.html").is_file());
    let robots = fs::read_to_string(out.join("robots.txt")).unwrap();
    assert_eq!(
        robots.trim(),
        "User-agent: *\nAllow: /\nSitemap: https://acme.example/sitemap.xml"
    );
    assert!(!out.join("robots.txt/index.html").exists());
    assert!(out.join("themes/acme/assets/css/site.css").is_file());
}

#[tokio::test]
async fn a_folder_without_a_theme_yaml_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("bare");
    fs::create_dir_all(dir.join("pages")).unwrap();
    let err = folder::load(&dir).await.unwrap_err();
    assert!(err.to_string().contains("theme.yaml"), "{err}");
}
