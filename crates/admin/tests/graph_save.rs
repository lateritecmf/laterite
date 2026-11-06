//! A form that edits one-to-one related records beside its own, saved in one
//! transaction. Imports only the public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use sea_query::Alias;
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";

struct CreateTables;

fn id() -> ColumnDef {
    ColumnDef::new(Alias::new("id"))
        .big_integer()
        .not_null()
        .auto_increment()
        .primary_key()
        .to_owned()
}

#[async_trait(?Send)]
impl Migration for CreateTables {
    fn name(&self) -> &str {
        "0001_create_tables"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("addresses"))
                .col(id())
                .col(ColumnDef::new(Alias::new("city")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("authors"))
                .col(id())
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(
                    ColumnDef::new(Alias::new("address_id"))
                        .big_integer()
                        .null(),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("profiles"))
                .col(id())
                .col(
                    ColumnDef::new(Alias::new("author_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(Alias::new("bio")).text().null())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new(
        "acme.graph",
        vec![Box::new(CreateTables)],
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
            resources: vec![resource!("tests/descriptors/graph_authors.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_authors",
                t!("Manage authors"),
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

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    session: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"));
    if method == "POST" {
        req = req
            .header("content-type", "application/x-www-form-urlencoded")
            .header("sec-fetch-site", "same-origin")
            .header("x-csrf-token", CSRF);
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Every row of `table` as `(id, column)` text, in id order.
async fn rows(db: &Db, table: &str, column: &str) -> Vec<(String, String)> {
    let sql = format!(
        "SELECT CAST(id AS {cast}), CAST({column} AS {cast}) FROM {table} ORDER BY id",
        cast = text_cast(db.backend)
    );
    sqlx::query_as(&sql).fetch_all(&db.pool).await.unwrap()
}

#[tokio::test]
async fn a_save_writes_the_record_and_its_one_to_one_records_together() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;

    let (status, body) = send(
        &app,
        "POST",
        "/admin/authors/new",
        &session,
        "name=Ada&bio=Mathematician&city=London",
    )
    .await;
    assert!(status.is_redirection(), "{status}: {body}");
    assert_eq!(
        rows(&db, "addresses", "city").await,
        [("1".to_string(), "London".to_string())]
    );
    assert_eq!(
        rows(&db, "authors", "address_id").await,
        [("1".to_string(), "1".to_string())]
    );
    assert_eq!(
        rows(&db, "profiles", "author_id").await,
        [("1".to_string(), "1".to_string())]
    );

    let (_, edit) = send(&app, "GET", "/admin/authors/1/edit", &session, "").await;
    assert!(
        edit.contains("Mathematician") && edit.contains("London"),
        "{edit}"
    );

    let (status, body) = send(
        &app,
        "POST",
        "/admin/authors/1/edit",
        &session,
        "name=Ada&bio=Analyst&city=Paris",
    )
    .await;
    assert!(status.is_redirection(), "{status}: {body}");
    assert_eq!(
        rows(&db, "addresses", "city").await,
        [("1".to_string(), "Paris".to_string())]
    );
    assert_eq!(
        rows(&db, "profiles", "bio").await,
        [("1".to_string(), "Analyst".to_string())]
    );
}

#[tokio::test]
async fn a_related_write_that_fails_rolls_the_record_back() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    // The address's city is required by the table; a blank one fails its insert.
    let (status, _) = send(
        &app,
        "POST",
        "/admin/authors/new",
        &session,
        "name=Ada&bio=Mathematician&city=",
    )
    .await;
    assert!(!status.is_redirection(), "{status}");
    assert!(
        rows(&db, "authors", "name").await.is_empty(),
        "nothing half-saved"
    );
    assert!(rows(&db, "profiles", "bio").await.is_empty());
}
