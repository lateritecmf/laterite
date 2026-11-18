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
async fn the_dashboard_frames_every_widget_and_each_fetches_itself() {
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
            html.contains(&format!(r#"data-id="{code}""#)),
            "{code} missing"
        );
        assert!(
            html.contains(&format!(r#"hx-get="/admin/dashboard/widgets/{code}""#)),
            "{code} does not fetch itself"
        );
    }
    // The page carries frames alone; a widget's content comes after.
    assert!(!html.contains("42 orders"));
    assert!(html.contains("Sales today"));
    assert!(
        html.contains("data-lat-board-start"),
        "the viewer may arrange it"
    );
    // A third sits at 7 columns in a row of thirds, 10 beside a half.
    let sales = &html[html.find(r#"data-id="acme.sales""#).unwrap()..];
    assert!(
        sales[..120].contains(r#"data-width="7""#) || sales[..120].contains(r#"data-width="10""#),
        "{}",
        &sales[..160]
    );

    let body = text(
        app(&db)
            .oneshot(get("/admin/dashboard/widgets/acme.sales", &token))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body.trim(), "<p>42 orders</p>");
    let broken = text(
        app(&db)
            .oneshot(get("/admin/dashboard/widgets/acme.broken", &token))
            .await
            .unwrap(),
    )
    .await;
    assert!(broken.contains("This widget could not be shown."));
    assert!(!broken.contains("the upstream is down"));
    let res = app(&db)
        .oneshot(get("/admin/dashboard/widgets/acme.nope", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_operator_keeps_their_own_arrangement_until_they_reset_it() {
    let (db, _guard) = test_db().await;
    let token = session(&db).await;
    // Sales first and full width, welcome under it; the rest are taken off.
    // A width past a kind's bounds and a widget the board never had are dropped.
    let layout = r#"[[{"id":"acme.sales","width":99},{"id":"nope","width":5}],[{"id":"backend.welcome","width":1}]]"#;
    let body = format!(
        "scope=mine&layout={}",
        layout.replace('"', "%22").replace(' ', "+")
    );
    let res = app(&db)
        .oneshot(post("/admin/dashboard/layout", &token, &body))
        .await
        .unwrap();
    assert!(res.status().is_redirection());
    let html = text(app(&db).oneshot(get("/admin", &token)).await.unwrap()).await;
    let sales = html.find(r#"data-id="acme.sales""#).unwrap();
    let welcome = html.find(r#"data-id="backend.welcome""#).unwrap();
    assert!(sales < welcome, "the saved order puts sales first");
    assert!(html[sales..sales + 120].contains(r#"data-width="20""#));
    assert!(
        html[welcome..welcome + 120].contains(r#"data-width="5""#),
        "a width stays within the kind's bounds"
    );
    assert!(
        !html.contains(r#"data-id="backend.status""#),
        "removed widgets stay off"
    );
    assert!(
        html.contains(r#"value="reset""#),
        "their own layout can be dropped"
    );

    // Adding one back puts it on the board and returns to edit mode.
    let res = app(&db)
        .oneshot(post(
            "/admin/dashboard/layout",
            &token,
            "add=backend.status",
        ))
        .await
        .unwrap();
    let to = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(to.contains("edit=1"), "{to}");
    let html = text(
        app(&db)
            .oneshot(get("/admin?edit=1", &token))
            .await
            .unwrap(),
    )
    .await;
    assert!(html.contains(r#"data-lat-board-edit="1""#));
    assert_eq!(html.matches(r#"data-kind-probe"#).count(), 0);
    let status = app(&db)
        .oneshot(get("/admin/dashboard/widgets/w1", &token))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK, "the added widget draws");
    // Each kind of home widget sits on the board once.
    app(&db)
        .oneshot(post(
            "/admin/dashboard/layout",
            &token,
            "add=backend.status",
        ))
        .await
        .unwrap();
    let html = text(app(&db).oneshot(get("/admin", &token)).await.unwrap()).await;
    assert!(!html.contains(r#"data-id="w2""#));

    let res = app(&db)
        .oneshot(post("/admin/dashboard/layout", &token, "scope=reset"))
        .await
        .unwrap();
    assert!(res.status().is_redirection());
    let html = text(app(&db).oneshot(get("/admin", &token)).await.unwrap()).await;
    assert!(html.contains(r#"data-id="backend.status""#));
    // Nobody may change what everyone sees on the home dashboard.
    let res = app(&db)
        .oneshot(post(
            "/admin/dashboard/layout",
            &token,
            "scope=shared&layout=[]",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
