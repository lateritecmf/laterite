//! A settings screen's form: fields on tabs, choices from an option source,
//! and Reset to defaults. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::FormField;
use laterite_admin::options::{OptionEntry, OptionSourceReg};
use laterite_admin::settings::SettingsItem;
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const PATH: &str = "/admin/settings/acme/shop";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            settings: vec![SettingsItem::new(
                "acme.shop",
                t!("Shop"),
                vec![
                    FormField::text("name", "Shop name"),
                    FormField::of("region", "Region", "select")
                        .options(serde_json::json!({ "options": "acme.regions" })),
                    FormField::text("footer", "Footer line").tab("Layout"),
                    FormField::text("columns", "Columns").tab("Layout"),
                    FormField::text("sender", "Sender").tab("Mail"),
                ],
            )
            .category("Shop")],
            option_sources: vec![OptionSourceReg::fixed(
                "acme.regions",
                vec![
                    OptionEntry::new("in", "India"),
                    OptionEntry::new("sg", "Singapore"),
                ],
            )],
            ..Default::default()
        },
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

async fn stored(db: &Db) -> serde_json::Value {
    laterite_admin::settings::get(db, "acme.shop")
        .await
        .unwrap()
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn fields_gather_on_their_tabs_and_a_source_fills_the_choices() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let root = session(&db).await;
    let (status, html) = get(&app, PATH, &root).await;
    assert_eq!(status, StatusCode::OK);
    // Two tabs, in the order they first appear, the first one open.
    assert!(html.contains("data-lat-widget=\"formtabs\""));
    let strip = html.split("role=\"tablist\"").nth(1).unwrap();
    let strip = strip.split("</div>").next().unwrap();
    assert!(strip.find("Layout").unwrap() < strip.find("Mail").unwrap());
    assert_eq!(strip.matches("role=\"tab\"").count(), 2);
    // An untabbed field sits above the set; a tabbed one inside its pane.
    let at = |needle: &str| {
        html.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    assert!(at("name=\"name\"") < at("data-lat-widget=\"formtabs\""));
    assert!(at("id=\"tab-layout-pane\"") < at("name=\"footer\""));
    assert!(at("name=\"columns\"") < at("id=\"tab-mail-pane\""));
    assert!(at("id=\"tab-mail-pane\"") < at("name=\"sender\""));
    // The select offers what its source returns.
    assert!(html.contains(">India<"));
    assert!(html.contains(">Singapore<"));

    // A save keeps every field, tabbed or not.
    let status = post(
        &app,
        PATH,
        &root,
        "name=Acme+Store&region=sg&footer=Thanks&columns=3&sender=shop%40acme.com",
    )
    .await;
    assert!(status.is_redirection());
    let value = stored(&db).await;
    assert_eq!(value["name"], "Acme Store");
    assert_eq!(value["region"], "sg");
    assert_eq!(value["footer"], "Thanks");
    assert_eq!(value["sender"], "shop@acme.com");
    let (_, html) = get(&app, PATH, &root).await;
    assert!(html.contains("value=\"sg\" selected") || html.contains("value=\"sg\"selected"));
}

#[tokio::test]
async fn reset_puts_every_field_back_to_its_default() {
    let (db, _g) = test_db().await;
    let app = app(&db);
    let root = session(&db).await;
    post(&app, PATH, &root, "name=Acme+Store&region=in&footer=Thanks").await;
    assert_eq!(stored(&db).await["name"], "Acme Store");
    let (_, html) = get(&app, PATH, &root).await;
    assert!(html.contains("Reset to defaults"));

    let status = post(&app, &format!("{PATH}/reset"), &root, "").await;
    assert!(status.is_redirection());
    assert_eq!(stored(&db).await, serde_json::json!({}));
    let (_, html) = get(&app, PATH, &root).await;
    assert!(html.contains("Back to the defaults."));
    assert!(!html.contains("Acme Store"));
    // Signed out, the reset is not served.
    let status = post(&app, &format!("{PATH}/reset"), "nobody", "").await;
    assert!(!status.is_success());
}
