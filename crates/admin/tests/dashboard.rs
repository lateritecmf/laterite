//! The dashboard shows the widgets an operator may see, in their own layout,
//! and a contributed widget joins the framework's. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::html::Markup;
use laterite_admin::widget::{Size, Widget, WidgetCx, WidgetReg};
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::{t, CatalogStore, Db, Text};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";

struct Sales;

#[laterite_core::strata::async_trait]
impl Widget for Sales {
    fn code(&self) -> &str {
        "acme.sales"
    }
    fn label(&self) -> Text {
        t!("Sales today")
    }
    fn size(&self) -> Size {
        Size::Third
    }
    async fn render(&self, _cx: &WidgetCx<'_>) -> Result<Markup, String> {
        Ok(Markup::raw_unchecked("<p>42 orders</p>"))
    }
}

struct Broken;

#[laterite_core::strata::async_trait]
impl Widget for Broken {
    fn code(&self) -> &str {
        "acme.broken"
    }
    fn label(&self) -> Text {
        t!("Broken")
    }
    async fn render(&self, _cx: &WidgetCx<'_>) -> Result<Markup, String> {
        Err("the upstream is down".into())
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            widgets: vec![
                WidgetReg::new(Arc::new(Sales)),
                WidgetReg::new(Arc::new(Broken)),
            ],
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
    svc.set_session_data(&token, &format!(r#"{{"v":1,"csrf":"{CSRF}"}}"#))
        .await
        .unwrap();
    token
}

fn get(path: &str, session: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, session: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("origin", ORIGIN)
        .header("x-csrf-token", CSRF)
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn the_dashboard_frames_every_widget_and_isolates_a_failure() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let res = app(&db).oneshot(get("/admin", &token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = text(res).await;
    for code in [
        "backend.welcome",
        "backend.manage",
        "backend.activity",
        "backend.status",
        "acme.sales",
        "acme.broken",
    ] {
        assert!(
            html.contains(&format!(r#"data-widget="{code}""#)),
            "{code} missing"
        );
    }
    assert!(html.contains("42 orders"));
    assert!(html.contains("This widget could not be shown."));
    assert!(!html.contains("the upstream is down"));
    assert!(html.contains(r#"id="dashboard-manage""#));
    // The contributed widget takes its own default width.
    assert!(html.contains(r#"lat-widget lat-widget--third" data-widget="acme.sales""#));
}

#[tokio::test]
async fn an_operator_keeps_their_own_arrangement_until_they_reset_it() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let res = app(&db)
        .oneshot(post(
            "/admin/dashboard/layout",
            &token,
            "widget=acme.sales&size%3Aacme.sales=full&widget=backend.welcome&widget=nope",
        ))
        .await
        .unwrap();
    assert!(res.status().is_redirection());
    let html = text(app(&db).oneshot(get("/admin", &token)).await.unwrap()).await;
    let sales = html.find(r#"data-widget="acme.sales""#).unwrap();
    let welcome = html.find(r#"data-widget="backend.welcome""#).unwrap();
    assert!(sales < welcome, "the saved order puts sales first");
    assert!(html.contains(r#"lat-widget lat-widget--full" data-widget="acme.sales""#));
    assert!(
        !html.contains(r#"data-widget="backend.status""#),
        "unticked widgets stay off"
    );
    let res = app(&db)
        .oneshot(post("/admin/dashboard/layout/reset", &token, ""))
        .await
        .unwrap();
    assert!(res.status().is_redirection());
    let html = text(app(&db).oneshot(get("/admin", &token)).await.unwrap()).await;
    assert!(html.contains(r#"data-widget="backend.status""#));
}

#[tokio::test]
async fn a_widget_fragment_serves_its_body_alone() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    let res = app(&db)
        .oneshot(get("/admin/dashboard/widget/acme.sales?size=full", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = text(res).await;
    assert_eq!(html.trim(), "<p>42 orders</p>");
    let res = app(&db)
        .oneshot(get("/admin/dashboard/widget/acme.nope", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
