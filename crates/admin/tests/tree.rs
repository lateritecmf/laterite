//! A list shown as a tree on its parent column: the top level, children
//! loading in place, the way down, and Move to. Imports only the public API.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use laterite_admin::picker::{PickerSourceReg, TableSource};
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

struct CreatePlaces;

#[async_trait(?Send)]
impl Migration for CreatePlaces {
    fn name(&self) -> &str {
        "0001_create_places"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("places"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("parent_id")).big_integer().null())
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
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
    sets.push(MigrationSet::new(
        "acme.places",
        vec![Box::new(CreatePlaces)],
    ));
    laterite_core::testing::connect_test(&sets).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/tree.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_places",
                t!("Manage places"),
                t!("Acme"),
            )],
            picker_sources: vec![PickerSourceReg::new(
                "acme.places",
                Arc::new(TableSource::new("places", "id", "name")),
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

fn get(path: &str, session: &str, htmx: bool) -> Request<Body> {
    let mut req = Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"));
    if htmx {
        req = req.header("hx-request", "true");
    }
    req.body(Body::empty()).unwrap()
}

fn post(path: &str, session: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", CSRF)
        .body(Body::from(body.to_string()))
        .unwrap()
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

/// 1 India (2 Karnataka (3 Bengaluru), 4 Kerala), 5 Nepal.
async fn places(db: &Db) {
    let rows: [(Option<i64>, &str, i64); 5] = [
        (None, "India", 1000),
        (Some(1), "Karnataka", 1000),
        (Some(2), "Bengaluru", 1000),
        (Some(1), "Kerala", 2000),
        (None, "Nepal", 2000),
    ];
    for (parent, name, key) in rows {
        let (sql, values) = build(
            db.backend,
            Query::insert()
                .into_table(Alias::new("places"))
                .columns([
                    Alias::new("parent_id"),
                    Alias::new("name"),
                    Alias::new("sort_order"),
                ])
                .values_panic([parent.into(), name.into(), key.into()])
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

/// The parent of a place, by id.
async fn parent_of(db: &Db, id: i64) -> Option<i64> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Alias::new("parent_id"))
            .from(Alias::new("places"))
            .and_where(sea_query::Expr::col(Alias::new("id")).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap()
        .get_int("parent_id")
        .ok()
}

#[tokio::test]
async fn the_list_opens_on_the_top_level_and_a_row_expands_in_place() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    places(&db).await;

    let (status, _, body) = send(&app, get("/admin/places", &session, false)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("India") && body.contains("Nepal"), "{body}");
    assert!(
        !body.contains("Karnataka"),
        "children wait for their toggle"
    );
    assert!(body.contains(r#"data-lat-widget="tree""#), "the island");
    assert!(
        body.contains(r#"href="/admin/places?parent=1&amp;depth=1""#)
            || body.contains(r#"href="/admin/places?parent=1&#38;depth=1""#),
        "India has a toggle: {body}"
    );
    assert!(
        body.contains(r#"href="/admin/places/5/move""#),
        "every row offers Move to"
    );

    // The toggle's request: the children alone, one level in.
    let (status, _, body) = send(&app, get("/admin/places?parent=1&depth=1", &session, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.trim_start().starts_with("<tr"), "rows alone: {body}");
    assert!(body.contains("Karnataka") && body.contains("Kerala") && !body.contains("India"));
    assert!(body.contains(r#"data-lat-depth="1""#) && body.contains("padding-left:34px"));
    assert!(
        body.contains(r#"name="parent" value="1""#),
        "a move among these stays among them"
    );

    // The same link with scripting off: a page of its own, with the way down.
    let (status, _, body) = send(&app, get("/admin/places?parent=2", &session, false)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Bengaluru"), "{body}");
    let all = body.find(">All<").unwrap();
    let india = body.find(">India<").unwrap();
    let karnataka = body.find(">Karnataka<").unwrap();
    assert!(all < india && india < karnataka, "the crumbs in order");

    // A search is a flat result.
    let (_, _, body) = send(&app, get("/admin/places?q=Ka", &session, false)).await;
    assert!(body.contains("Karnataka") && !body.contains(r#"data-lat-widget="tree""#));
}

#[tokio::test]
async fn a_row_moves_under_another_parent_but_never_under_itself() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    places(&db).await;

    let (status, _, body) = send(&app, get("/admin/places/4/move", &session, false)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"data-lat-widget="ref-picker""#) && body.contains(r#"value="1""#),
        "the picker, holding the current parent: {body}"
    );

    // Kerala under Nepal.
    let (status, location, _) =
        send(&app, post("/admin/places/4/move", &session, "parent_id=5")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/places?parent=5"));
    assert_eq!(parent_of(&db, 4).await, Some(5));

    // India under Bengaluru would close a loop; India under India too.
    let (status, _, body) = send(&app, post("/admin/places/1/move", &session, "parent_id=3")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("under itself"));
    let (status, _, _) = send(&app, post("/admin/places/1/move", &session, "parent_id=1")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(parent_of(&db, 1).await, None);

    // A parent that is not there, and the top level.
    let (status, _, body) =
        send(&app, post("/admin/places/3/move", &session, "parent_id=99")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("no such record"));
    let (status, location, _) =
        send(&app, post("/admin/places/3/move", &session, "parent_id=")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/places"));
    assert_eq!(parent_of(&db, 3).await, None);
    let (_, _, body) = send(&app, get("/admin/places", &session, false)).await;
    assert!(body.contains("Bengaluru"), "now at the top level");
}

#[tokio::test]
async fn a_move_among_children_stays_among_them() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    places(&db).await;

    // Kerala above Karnataka, under India; Nepal at the top is untouched.
    let (status, _, body) = send(
        &app,
        post(
            "/admin/places/reorder",
            &session,
            "id=4&move=up&parent=1&page=1",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let (_, _, body) = send(&app, get("/admin/places?parent=1&depth=1", &session, true)).await;
    let kerala = body.find("Kerala").expect(&body);
    let karnataka = body.find("Karnataka").expect(&body);
    assert!(kerala < karnataka, "{body}");
    let (status, location, body) = send(&app, get("/admin/places", &session, false)).await;
    assert_eq!(status, StatusCode::OK, "{location:?} {body}");
    let india = body.find("India").unwrap();
    let nepal = body.find("Nepal").unwrap();
    assert!(india < nepal, "the top level keeps its order");
}
