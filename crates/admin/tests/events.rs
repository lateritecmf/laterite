//! The event bus as a module meets it: a route announces, a listener hears, and
//! what the panel does reaches listeners too. Imports only the public API.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use laterite_admin::routes::{PublicRoute, PublicRouteReg, RouteCtx};
use laterite_admin::{router, AdminConfig, Contributions};
use laterite_auth::events::{SignedIn, SignedOut};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::async_trait;
use laterite_core::{CatalogStore, Db, Event, EventCx, EventError, Events, Listener};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

#[derive(Serialize, Deserialize)]
struct Pinged {
    from: String,
}

impl Event for Pinged {
    const NAME: &'static str = "acme.hooks.pinged";
}

type Heard = Arc<Mutex<Vec<String>>>;

struct Recorder(Heard);

#[async_trait]
impl Listener<Pinged> for Recorder {
    async fn handle(&self, cx: &EventCx<'_>, event: &Pinged) -> Result<(), EventError> {
        // The context carries the pool the application runs on.
        sqlx::query("SELECT 1").execute(&cx.db().pool).await?;
        self.0
            .lock()
            .unwrap()
            .push(format!("pinged by {}", event.from));
        Ok(())
    }
}

#[async_trait]
impl Listener<SignedIn> for Recorder {
    async fn handle(&self, _cx: &EventCx<'_>, event: &SignedIn) -> Result<(), EventError> {
        self.0
            .lock()
            .unwrap()
            .push(format!("{} signed in", event.username));
        Ok(())
    }
}

#[async_trait]
impl Listener<SignedOut> for Recorder {
    async fn handle(&self, _cx: &EventCx<'_>, event: &SignedOut) -> Result<(), EventError> {
        self.0
            .lock()
            .unwrap()
            .push(format!("{} signed out", event.user_id));
        Ok(())
    }
}

/// A webhook receiver: announces every call it takes.
struct Hook;

impl PublicRoute for Hook {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let events = ctx.events().clone();
        Router::new().route(
            "/",
            post(move || {
                let events = events.clone();
                async move {
                    events
                        .emit(&Pinged {
                            from: "acme".to_string(),
                        })
                        .await;
                    StatusCode::NO_CONTENT
                }
            }),
        )
    }
}

fn listening(db: &Db) -> (Events, Heard) {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let events = Events::builder(db.clone())
        .listen::<Pinged>(Recorder(heard.clone()))
        .listen::<SignedIn>(Recorder(heard.clone()))
        .listen::<SignedOut>(Recorder(heard.clone()))
        .build()
        .unwrap();
    (events, heard)
}

fn app(db: Db, events: Events) -> Router {
    let auth = AuthService::new(db.clone(), AuthConfig::default()).with_events(events);
    router(
        auth,
        db,
        Contributions {
            public_routes: vec![PublicRouteReg::new("/hooks/acme", Arc::new(Hook))],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    )
}

#[tokio::test]
async fn a_route_announces_on_the_bus_the_application_was_given() {
    let (db, _guard) = test_db().await;
    let (events, heard) = listening(&db);

    let resp = app(db, events)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/hooks/acme")
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(*heard.lock().unwrap(), ["pinged by acme"]);
}

#[tokio::test]
async fn a_route_is_tested_against_a_bus_built_by_hand() {
    let (db, _guard) = test_db().await;
    let (events, heard) = listening(&db);
    let ctx = RouteCtx::builder(db).events(events).build();

    let resp = Hook
        .mount(&ctx)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(*heard.lock().unwrap(), ["pinged by acme"]);
}

#[tokio::test]
async fn signing_out_of_the_panel_reaches_a_listener() {
    let (db, _guard) = test_db().await;
    let (events, heard) = listening(&db);
    let svc = AuthService::new(db.clone(), AuthConfig::default()).with_events(events.clone());
    let id = svc
        .create_superuser(NewOperator {
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

    // The panel's own page, for the token its forms carry.
    let page = app(db.clone(), events.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/admin")
                .header("cookie", format!("{SESSION_COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = axum::body::to_bytes(page.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(html.to_vec()).unwrap();
    let csrf = html
        .split("name=\"_csrf\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();

    let resp = app(db, events)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/logout")
                .header("cookie", format!("{SESSION_COOKIE}={token}"))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("_csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(resp.status().is_redirection(), "{}", resp.status());
    assert_eq!(
        *heard.lock().unwrap(),
        ["root signed in".to_string(), format!("{id} signed out")]
    );
}
