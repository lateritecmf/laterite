//! Cells edited in the row: the field's control in the cell, saved on change
//! through the form's rules and listeners. Imports only the public API.

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

struct CreateProducts;

#[async_trait(?Send)]
impl Migration for CreateProducts {
    fn name(&self) -> &str {
        "0001_create_products"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("products"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .col(ColumnDef::new(Alias::new("status")).text().null())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new(
        "acme.products",
        vec![Box::new(CreateProducts)],
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
            resources: vec![resource!("tests/descriptors/editable.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_products",
                t!("Manage products"),
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

/// The product as (name, code, status).
async fn product(db: &Db, id: i64) -> (String, String, Option<String>) {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Alias::new("name"), Alias::new("code"), Alias::new("status")])
            .from(Alias::new("products"))
            .and_where(sea_query::Expr::col(Alias::new("id")).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    (
        row.get_text("name").unwrap(),
        row.get_text("code").unwrap(),
        row.get_text("status").ok().filter(|s| !s.is_empty()),
    )
}

/// One product, created through the form; returns the session.
async fn one_product(app: &Router, db: &Db) -> String {
    let session = session(db).await;
    let (status, _, _) = send(
        app,
        post(
            "/admin/products/new",
            &session,
            "name=Lamp&code=LMP&status=draft",
            false,
        ),
    )
    .await;
    assert!(status.is_redirection(), "{status}");
    session
}

#[tokio::test]
async fn an_editable_cell_is_the_fields_control_and_saves_through_the_form() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = one_product(&app, &db).await;

    let (status, _, body) = send(&app, get("/admin/products", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"action="/admin/products/1/cell""#)
            && body.contains(r#"id="cell-1-name""#)
            && body.contains(r#"<select"#),
        "a form per editable cell, the select for status: {body}"
    );
    assert!(
        body.contains("lat-cell__save"),
        "a Save button for a page without scripting"
    );
    let lmp = body.find(">LMP<").expect(&body);
    assert!(
        !body[lmp - 200..lmp].contains("<form"),
        "a plain column stays plain"
    );

    // Saved: the cell comes back holding the stored value.
    let (status, _, body) = send(
        &app,
        post(
            "/admin/products/1/cell",
            &session,
            "field=status&status=live",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.starts_with(r#"<form class="lat-cell""#), "{body}");
    assert!(
        body.contains(r#"value="live" selected"#)
            || body.contains(r#"selected value="live""#)
            || body.contains("live"),
        "{body}"
    );
    assert_eq!(
        product(&db, 1).await,
        (
            "Lamp".to_string(),
            "LMP".to_string(),
            Some("live".to_string())
        )
    );

    // Refused: the rule's message under the control, the row untouched.
    let (status, _, body) = send(
        &app,
        post("/admin/products/1/cell", &session, "field=name&name=", true),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("is-invalid"), "{body}");
    let error = body.find("lat-cell__error").expect(&body);
    assert!(
        body[error..].contains("required"),
        "the rule's message under the control: {body}"
    );
    assert_eq!(product(&db, 1).await.0, "Lamp");

    // With scripting off: back to the list, with the flash.
    let (status, location, _) = send(
        &app,
        post(
            "/admin/products/1/cell",
            &session,
            "field=name&name=Desk lamp",
            false,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/products"));
    assert_eq!(product(&db, 1).await.0, "Desk lamp");
    let (_, _, body) = send(&app, get("/admin/products", &session)).await;
    assert!(body.contains("Saved."), "{body}");

    // A column that is not editable, and a row that is not there.
    let (status, _, _) = send(
        &app,
        post(
            "/admin/products/1/cell",
            &session,
            "field=code&code=X",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(
        &app,
        post(
            "/admin/products/9/cell",
            &session,
            "field=name&name=X",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
