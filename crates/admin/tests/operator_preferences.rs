//! A module's per-operator settings: one typed model, stored per operator,
//! edited from their Preferences page. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::FormField;
use laterite_admin::settings::{load_for, save_for, PreferencesModel, SettingsItem};
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
struct Desk {
    #[serde(default)]
    greeting: String,
    #[serde(default)]
    compact: bool,
}

impl PreferencesModel for Desk {
    const CODE: &'static str = "acme.desk";
}

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
                "acme.desk",
                t!("My desk"),
                vec![
                    FormField::text("greeting", "Greeting"),
                    FormField::switch("compact", "Compact rows"),
                ],
            )
            .description("How the desk greets you.")
            .per_operator()],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// Two operators with no permissions at all: preferences need none.
async fn sessions(db: &Db) -> (String, i64, String, i64) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let hash = password::hash_password("pw1234567890").unwrap();
    let a = store::create_user(db, "ann", "ann@acme.test", "Ann", None, &hash, false)
        .await
        .unwrap();
    let b = store::create_user(db, "bob", "bob@acme.test", "Bob", None, &hash, false)
        .await
        .unwrap();
    let mut tokens = Vec::new();
    for user in ["ann", "bob"] {
        let token = svc
            .authenticate(user, "pw1234567890", &RequestContext::default())
            .await
            .unwrap()
            .token;
        svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
            .await
            .unwrap();
        tokens.push(token);
    }
    let _ = NewOperator {
        username: "",
        email: "",
        first_name: "",
        last_name: None,
        password: "",
        timezone: None,
    };
    (tokens.remove(0), a, tokens.remove(0), b)
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

async fn post(app: &Router, path: &str, session: &str, body: &str) -> (StatusCode, Option<String>) {
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
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (response.status(), location)
}

#[tokio::test]
async fn each_operator_keeps_their_own_copy_of_a_per_operator_item() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let (ann, ann_id, bob, bob_id) = sessions(&db).await;

    // Listed on Preferences, not on Settings; its page is under Preferences.
    let (status, html) = get(&app, "/admin/preferences", &ann).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("My desk") && html.contains(r#"href="/admin/preferences/acme/desk""#),
        "{html}"
    );
    let (_, html) = get(&app, "/admin/settings", &ann).await;
    assert!(!html.contains("My desk"), "not a settings screen: {html}");
    let (status, html) = get(&app, "/admin/preferences/acme/desk", &ann).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"name="greeting""#), "{html}");

    // Ann saves; Bob still sees the defaults; the typed model reads back.
    let (status, location) = post(
        &app,
        "/admin/preferences/acme/desk",
        &ann,
        "greeting=Hello+Ann&compact=on",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/preferences/acme/desk"));
    assert_eq!(
        load_for::<Desk>(&db, ann_id).await.unwrap(),
        Desk {
            greeting: "Hello Ann".to_string(),
            compact: true
        }
    );
    assert_eq!(
        load_for::<Desk>(&db, bob_id).await.unwrap(),
        Desk::default()
    );
    let (_, html) = get(&app, "/admin/preferences/acme/desk", &bob).await;
    assert!(!html.contains("Hello Ann"), "{html}");

    // The typed write shows on the page.
    save_for(
        &db,
        bob_id,
        &Desk {
            greeting: "Hi Bob".to_string(),
            compact: false,
        },
    )
    .await
    .unwrap();
    let (_, html) = get(&app, "/admin/preferences/acme/desk", &bob).await;
    assert!(html.contains("Hi Bob"), "{html}");
}
