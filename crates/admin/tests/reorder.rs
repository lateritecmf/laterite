//! Rows kept in an order of their own: the grip and Move buttons, the posted
//! move, sparse keys and the renumber. Imports only the public API.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use sea_query::{Alias, Query};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";

struct CreateSteps;

#[async_trait(?Send)]
impl Migration for CreateSteps {
    fn name(&self) -> &str {
        "0001_create_steps"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("steps"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .col(
                    ColumnDef::new(Alias::new("sort_order"))
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.steps", vec![Box::new(CreateSteps)]));
    laterite_core::testing::connect_test(&sets).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/reorder.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_steps",
                t!("Manage steps"),
                t!("Acme"),
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

fn post(path: &str, session: &str, body: &str, htmx: bool) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", CSRF);
    if htmx {
        req = req.header("hx-request", "true");
    }
    req.body(Body::from(body.to_string())).unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Option<String>, String) {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&bytes).to_string(),
    )
}

/// Steps written straight to the table with the given keys.
async fn steps(db: &Db, rows: &[(&str, i64)]) {
    for (title, key) in rows {
        let (sql, values) = build(
            db.backend,
            Query::insert()
                .into_table(Alias::new("steps"))
                .columns([Alias::new("title"), Alias::new("sort_order")])
                .values_panic([(*title).into(), (*key).into()])
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

/// The steps as (title, key), in the list's own order.
async fn order(db: &Db) -> Vec<(String, i64)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Alias::new("title"), Alias::new("sort_order")])
            .from(Alias::new("steps"))
            .order_by(Alias::new("sort_order"), sea_query::Order::Asc)
            .order_by(Alias::new("id"), sea_query::Order::Asc)
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get_text("title").unwrap(),
                row.get_int("sort_order").unwrap(),
            )
        })
        .collect()
}

fn titles(order: &[(String, i64)]) -> Vec<&str> {
    order.iter().map(|(t, _)| t.as_str()).collect()
}

#[tokio::test]
async fn the_list_opens_in_its_own_order_with_a_grip_and_arrows() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    steps(&db, &[("Brief", 3000), ("Design", 1000), ("Build", 2000)]).await;

    let (status, _, body) = send(&app, get("/admin/steps", &session)).await;
    assert_eq!(status, StatusCode::OK);
    let design = body.find("Design").unwrap();
    let build = body.find("Build").unwrap();
    let brief = body.find("Brief").unwrap();
    assert!(design < build && build < brief, "the list's own order");
    assert!(
        body.contains(r#"data-lat-widget="reorder""#) && body.contains(r#"name="move" value="up""#),
        "the grip island and the arrows: {body}"
    );
    assert!(
        body.contains(r#"value="up" aria-label="Move up" disabled"#)
            || body.contains(r#"value="up" aria-label='Move up' disabled"#),
        "the first row cannot move up"
    );

    // Sorted by another column, nothing moves.
    let (_, _, body) = send(&app, get("/admin/steps?sort=title&dir=asc", &session)).await;
    assert!(!body.contains(r#"data-lat-widget="reorder""#));
    let (_, _, body) = send(&app, get("/admin/steps?q=Bu", &session)).await;
    assert!(!body.contains(r#"data-lat-widget="reorder""#));
}

#[tokio::test]
async fn a_move_rewrites_one_key_when_a_gap_holds_it() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    steps(&db, &[("Design", 1000), ("Build", 2000), ("Brief", 3000)]).await;

    // Brief up one: between Design and Build, in the gap.
    let (status, location, _) = send(
        &app,
        post(
            "/admin/steps/reorder",
            &session,
            "id=3&move=up&page=1",
            false,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/steps?page=1"));
    let now = order(&db).await;
    assert_eq!(titles(&now), ["Design", "Brief", "Build"]);
    assert_eq!(now[1].1, 1500, "the midpoint of the gap");
    assert_eq!((now[0].1, now[2].1), (1000, 2000), "the others untouched");

    // Design to the end, past the last key.
    send(
        &app,
        post(
            "/admin/steps/reorder",
            &session,
            "id=1&after=2&page=1",
            false,
        ),
    )
    .await;
    let now = order(&db).await;
    assert_eq!(titles(&now), ["Brief", "Build", "Design"]);
    assert_eq!(now[2].1, 3000);

    // Design back to the front, before the first key.
    send(
        &app,
        post(
            "/admin/steps/reorder",
            &session,
            "id=1&before=3&page=1",
            false,
        ),
    )
    .await;
    let now = order(&db).await;
    assert_eq!(titles(&now), ["Design", "Brief", "Build"]);
    assert_eq!(now[0].1, 500);

    // An htmx move answers with the region itself, in the new order.
    let (status, _, body) = send(
        &app,
        post(
            "/admin/steps/reorder",
            &session,
            "id=2&move=up&page=1",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with(r#"<div id="lat-list""#), "{body}");
    assert_eq!(titles(&order(&db).await), ["Design", "Build", "Brief"]);
}

#[tokio::test]
async fn a_move_into_a_closed_gap_renumbers_the_list() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    steps(&db, &[("One", 1), ("Two", 2), ("Three", 3), ("Four", 4)]).await;

    send(
        &app,
        post(
            "/admin/steps/reorder",
            &session,
            "id=4&before=2&page=1",
            false,
        ),
    )
    .await;
    let now = order(&db).await;
    assert_eq!(titles(&now), ["One", "Four", "Two", "Three"]);
    assert_eq!(
        now.iter().map(|(_, k)| *k).collect::<Vec<_>>(),
        [1000, 2000, 3000, 4000],
        "spaced out again"
    );

    // A row no longer there is reported, not an error; the order holds.
    let (status, _, _) = send(
        &app,
        post(
            "/admin/steps/reorder",
            &session,
            "id=9&move=down&page=1",
            false,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, _, body) = send(&app, get("/admin/steps", &session)).await;
    assert!(body.contains("no longer in the list"), "{body}");
    assert_eq!(titles(&order(&db).await), ["One", "Four", "Two", "Three"]);
}
