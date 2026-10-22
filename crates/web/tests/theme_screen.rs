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
    contributions.settings.push(theme::admin::settings_item());
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
    // Themes is a settings entry, not a tab: the settings index lists it for
    // an operator who may manage themes and leaves it out for one who may not.
    let (_, html) = get(&app, "/admin/settings", &plain).await;
    assert!(
        !html.contains("/admin/themes"),
        "out of their settings: {html}"
    );
    let (_, html) = get(&app, "/admin/settings", &root).await;
    assert!(
        html.contains("/admin/themes") && html.contains("Website"),
        "in settings: {html}"
    );
    let (_, html) = get(&app, "/admin", &root).await;
    assert!(
        !html.contains(r#"class="lat-topnav__section" href="/admin/themes""#),
        "not a tab: {html}"
    );
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
        html.contains("Layouts") && html.contains("Edit properties"),
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
        "url=%2F&markup=%7B%25+if+%25%7D",
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
        "url=%2F&markup=%3Ch1%3E%7B%7B+theme.name+%7D%7D%3C%2Fh1%3E",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = store::template(&db, t.id).await.unwrap().unwrap();
    assert_eq!(saved.markup, "<h1>{{ theme.name }}</h1>");
    assert_eq!(saved.settings["url"], "/");
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

#[tokio::test]
async fn the_theme_page_shows_and_saves_the_declared_settings() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    store::create_theme(
        &db,
        "acme",
        "name: Acme\nform:\n  fields:\n    site_name:\n      label: Site name\n      type: text\n",
    )
    .await
    .unwrap();

    let (status, html) = get(&app, "/admin/themes/acme", &root).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("Site name") && html.contains(r#"name="site_name""#),
        "{html}"
    );

    let (status, location) = post(
        &app,
        "/admin/themes/acme/settings",
        &root,
        "site_name=Acme+Site",
    )
    .await;
    assert_eq!(
        (status, location.as_str()),
        (StatusCode::SEE_OTHER, "/admin/themes/acme")
    );
    let theme = store::find_theme(&db, "acme").await.unwrap().unwrap();
    assert_eq!(
        theme.settings,
        serde_json::json!({"site_name": "Acme Site"})
    );
    let (_, html) = get(&app, "/admin/themes/acme", &root).await;
    assert!(
        html.contains("Theme settings saved.") && html.contains("Acme Site"),
        "{html}"
    );
}

const BOUNDARY: &str = "lat-boundary";

/// A multipart body: text fields, then one file field holding `file`.
fn multipart(fields: &[(&str, &str)], file: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in fields {
        out.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    }
    out.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"t.zip\"\r\nContent-Type: application/zip\r\n\r\n")
            .as_bytes(),
    );
    out.extend_from_slice(file);
    out.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    out
}

#[tokio::test]
async fn a_theme_exports_as_a_zip_and_imports_as_a_new_one() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    let base = store::create_theme(&db, "base", "name: Base\n")
        .await
        .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "home.htm",
        None,
        "<h1>hi</h1>",
        None,
    )
    .await
    .unwrap();

    let req = Request::builder()
        .uri("/admin/themes/base/export")
        .header("cookie", format!("{SESSION_COOKIE}={root}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/zip")
    );
    assert!(response
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .contains("base.zip"));
    let bytes = laterite_admin::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/admin/themes/import")
        .header("cookie", format!("{SESSION_COOKIE}={root}"))
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(&[("code", "copy")], &bytes)))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/admin/themes/copy")
    );
    let copy = store::find_theme(&db, "copy").await.unwrap().unwrap();
    assert_eq!(copy.config.name, "Base");
    let home = store::find_template(&db, copy.id, Kind::Page, "home.htm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(home.markup, "<h1>hi</h1>");
    let (_, html) = get(&app, "/admin/themes/copy", &root).await;
    assert!(
        html.contains("Imported 1 template(s), skipped 0."),
        "{html}"
    );
}

#[tokio::test]
async fn a_page_save_checks_its_url_and_layout() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    let theme = store::create_theme(&db, "acme", "name: Acme\n")
        .await
        .unwrap();
    let (_, location) = post(
        &app,
        "/admin/themes/acme/templates",
        &root,
        "kind=page&path=about.htm",
    )
    .await;
    let t = store::find_template(&db, theme.id, Kind::Page, "about.htm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        t.settings,
        serde_json::json!({"url": "/about"}),
        "a new page gets a url"
    );
    let editor = location;

    // The editor shows the page's settings as fields.
    let (_, html) = get(&app, &editor, &root).await;
    assert!(
        html.contains(r#"name="url""#) && html.contains(r#"name="hidden""#),
        "{html}"
    );
    assert!(!html.contains(r#"name="settings""#), "{html}");

    // A layout the theme lacks is refused; a bad pattern too.
    let (_, _) = post(
        &app,
        &format!("{editor}/save"),
        &root,
        "url=%2Fabout&layout=default&markup=x",
    )
    .await;
    let (_, html) = get(&app, &editor, &root).await;
    assert!(html.contains("no layout"), "{html}");
    assert_eq!(
        store::template(&db, t.id).await.unwrap().unwrap().markup,
        ""
    );

    let (_, _) = post(&app, &format!("{editor}/save"), &root, "url=about&markup=x").await;
    let (_, html) = get(&app, &editor, &root).await;
    assert!(html.contains("starts with a slash"), "{html}");

    store::save_template(
        &db,
        theme.id,
        Kind::Layout,
        "default.htm",
        None,
        "{% block page %}{% endblock %}",
        None,
    )
    .await
    .unwrap();
    let (_, html) = get(&app, &editor, &root).await;
    assert!(
        html.contains(r#"<option value="default""#),
        "the layout picker lists it: {html}"
    );
    let (_, _) = post(
        &app,
        &format!("{editor}/save"),
        &root,
        "url=%2Fabout&layout=default&title=About+us&hidden=1&markup=x",
    )
    .await;
    let saved = store::template(&db, t.id).await.unwrap().unwrap();
    assert_eq!(saved.markup, "x");
    assert_eq!(saved.settings["url"], "/about");
    assert_eq!(saved.settings["layout"], "default");
    assert_eq!(saved.settings["title"], "About us");
    assert_eq!(saved.settings["hidden"], true);
    let (_, html) = get(&app, "/admin/themes/acme", &root).await;
    assert!(
        html.contains(">/about</span>"),
        "the page's URL beside its path: {html}"
    );
}

/// The properties dialog rewrites the human fields of theme.yaml and keeps the
/// rest: the settings form the theme declares survives a rename.
#[tokio::test]
async fn the_properties_dialog_keeps_the_rest_of_the_config() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    store::create_theme(
        &db,
        "acme",
        "name: Acme\nform:\n  fields:\n    site_name:\n      label: Site name\n      type: text\n",
    )
    .await
    .unwrap();

    let (status, location) = post(
        &app,
        "/admin/themes/acme/properties",
        &root,
        "name=Acme+Clay&author=Acme+Studio&homepage=https%3A%2F%2Facme.test&description=Honest+clay&preview=https%3A%2F%2Facme.test%2Fpreview.png",
    )
    .await;
    assert_eq!(
        (status, location.as_str()),
        (StatusCode::SEE_OTHER, "/admin/themes/acme")
    );
    let theme = store::find_theme(&db, "acme").await.unwrap().unwrap();
    assert_eq!(theme.config.name, "Acme Clay");
    assert_eq!(theme.config.author, "Acme Studio");
    assert_eq!(
        theme.config.preview.as_deref(),
        Some("https://acme.test/preview.png")
    );
    assert!(theme.config.form.is_some(), "the settings form survives");

    let (_, html) = get(&app, "/admin/themes", &root).await;
    assert!(
        html.contains("https://acme.test/preview.png") && html.contains("Acme Clay"),
        "{html}"
    );
    let (status, _) = post(&app, "/admin/themes/acme/properties", &root, "name=").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, html) = get(&app, "/admin/themes/acme", &root).await;
    assert!(html.contains("A theme needs a name."), "{html}");
}

/// A partial declares its snippet through fields, not a JSON box: the
/// declaration lands under `snippet`, a blank code removes it, and a bad code
/// is refused without a save.
#[tokio::test]
async fn a_partial_declares_its_snippet_through_the_form() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let (root, _) = sessions(&db).await;
    post(&app, "/admin/themes", &root, "code=acme&name=Acme+Site").await;
    let created = store::find_theme(&db, "acme").await.unwrap().unwrap();
    post(
        &app,
        "/admin/themes/acme/templates",
        &root,
        "kind=partial&path=snippets%2Fyoutube.htm",
    )
    .await;
    let t = store::find_template(&db, created.id, Kind::Partial, "snippets/youtube.htm")
        .await
        .unwrap()
        .unwrap();
    let editor = format!("/admin/themes/acme/templates/{}", t.id);
    let (_, html) = get(&app, &editor, &root).await;
    assert!(
        html.contains("Snippet")
            && html.contains("name=\"code\"")
            && !html.contains("name=\"settings\""),
        "{html}"
    );
    let body = concat!(
        "markup=%3Ciframe%3E&code=youtube&name=YouTube+video&description=Embeds+a+video",
        "&properties%5B0%5D%5Bcode%5D=url&properties%5B0%5D%5Btitle%5D=Video+URL&properties%5B0%5D%5Btype%5D=string",
        "&properties%5B1%5D%5Bcode%5D=autoplay&properties%5B1%5D%5Btitle%5D=Autoplay&properties%5B1%5D%5Btype%5D=checkbox&properties%5B1%5D%5Bdefault%5D=true",
        "&properties%5B2%5D%5Bcode%5D=size&properties%5B2%5D%5Btitle%5D=Size&properties%5B2%5D%5Btype%5D=dropdown&properties%5B2%5D%5Boptions%5D=sm%3ASmall+%7C+lg%3ALarge&properties%5B2%5D%5Bdefault%5D=sm"
    );
    let (status, _) = post(&app, &format!("{editor}/save"), &root, body).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = store::template(&db, t.id).await.unwrap().unwrap();
    let snippet = &saved.settings["snippet"];
    assert_eq!(snippet["code"], "youtube");
    assert_eq!(snippet["name"], "YouTube video");
    assert_eq!(snippet["description"], "Embeds a video");
    assert_eq!(snippet["properties"][0]["code"], "url");
    assert_eq!(snippet["properties"][1]["default"], true);
    assert_eq!(snippet["properties"][2]["options"], "sm:Small | lg:Large");
    assert_eq!(snippet["properties"][2]["default"], "sm");
    let (_, html) = get(&app, &editor, &root).await;
    assert!(
        html.contains("name=\"properties[0][code]\"") && html.contains("value=\"url\""),
        "{html}"
    );
    // A code that is not a code is refused and nothing changes.
    let (status, _) = post(
        &app,
        &format!("{editor}/save"),
        &root,
        "markup=%3Ciframe%3E&code=bad+code&name=X",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let kept = store::template(&db, t.id).await.unwrap().unwrap();
    assert_eq!(kept.settings["snippet"]["code"], "youtube");
    // A blank code withdraws the snippet.
    let (status, _) = post(
        &app,
        &format!("{editor}/save"),
        &root,
        "markup=%3Ciframe%3E&code=",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let withdrawn = store::template(&db, t.id).await.unwrap().unwrap();
    assert!(
        withdrawn.settings.get("snippet").is_none(),
        "{}",
        withdrawn.settings
    );
}
