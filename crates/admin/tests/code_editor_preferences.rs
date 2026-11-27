//! An operator.s own code editor preferences reach every page they open.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::settings::code_editor::CodeEditorPreferences;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions::default(),
        config,
        Arc::new(CatalogStore::default()),
    )
}

async fn session(db: &Db) -> String {
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
    let token = svc
        .authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    token
}

async fn get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn post(app: &Router, path: &str, session: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn saved_preferences_ride_on_every_page_for_the_editor_to_read() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let root = session(&db).await;

    // Nothing saved: the page carries nothing and the editor uses its defaults.
    let (_, html) = get(&app, "/admin/preferences", &root).await;
    assert!(!html.contains("data-lat-code-editor"));
    let link = html
        .split("href=\"")
        .map(|rest| rest.split('"').next().unwrap_or(""))
        .find(|href| href.contains("code_editor") || href.contains("code-editor"))
        .unwrap_or_else(|| panic!("no link to the item: {html}"))
        .to_string();

    // The form opens with the defaults.
    let (status, form) = get(&app, &link, &root).await;
    assert_eq!(status, StatusCode::OK);
    let at = form.find("name=\"line_numbers\"").expect(&form);
    let tag = &form[form[..at].rfind('<').unwrap()..];
    assert!(tag[..tag.find('>').unwrap()].contains("checked"));

    let status = post(
        &app,
        &link,
        &root,
        "font_size=16&tab_size=4&word_wrap=on&line_numbers=on",
    )
    .await;
    assert!(status.is_redirection());
    let saved: CodeEditorPreferences = laterite_admin::settings::load_for(&db, 1).await.unwrap();
    assert_eq!(saved.font_size, "16");
    assert!(saved.word_wrap && saved.line_numbers && !saved.code_folding);

    let (_, html) = get(&app, "/admin", &root).await;
    let body = html.split("data-lat-code-editor=\"").nth(1).expect(&html);
    let json = body
        .split('"')
        .next()
        .unwrap()
        .replace("&quot;", "\"")
        .replace("&#34;", "\"");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["tab_size"], "4");
    assert_eq!(value["word_wrap"], true);
    assert_eq!(value["code_folding"], false);
}
