//! Soft delete: Delete moves a record to the trash with its related rows, the
//! trash view restores or purges, and a contributed query hook shapes every
//! read. Imports only the public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet, QueryHook, QueryHookReg, ScopeCx};
use sea_query::{Alias, SelectStatement};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";

struct CreateNotes;

#[async_trait(?Send)]
impl Migration for CreateNotes {
    fn name(&self) -> &str {
        "0001_create_notes"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        let id = || {
            ColumnDef::new(Alias::new("id"))
                .big_integer()
                .not_null()
                .auto_increment()
                .primary_key()
                .to_owned()
        };
        s.exec(
            Table::create()
                .table(Alias::new("notes"))
                .col(id())
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .col(
                    ColumnDef::new(Alias::new("created_user_id"))
                        .big_integer()
                        .null(),
                )
                .col(
                    ColumnDef::new(Alias::new("updated_user_id"))
                        .big_integer()
                        .null(),
                )
                .col(soft_delete_col())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("comments"))
                .col(id())
                .col(
                    ColumnDef::new(Alias::new("note_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(Alias::new("body")).text().not_null())
                .col(soft_delete_col())
                .to_owned(),
        )
        .await
    }
}

/// Hides notes whose title starts with "Draft".
struct NoDrafts;

impl QueryHook for NoDrafts {
    fn name(&self) -> &str {
        "acme.no_drafts"
    }
    fn apply(&self, cx: &ScopeCx<'_>, select: &mut SelectStatement) {
        select
            .and_where(Expr::col((Alias::new(cx.entity), Alias::new("title"))).not_like("Draft%"));
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.notes", vec![Box::new(CreateNotes)]));
    laterite_core::testing::connect_test(&sets).await
}

fn app(db: &Db) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/trash_notes.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_notes",
                t!("Manage notes"),
                t!("Acme"),
            )],
            query_hooks: vec![QueryHookReg::for_entity("notes", Arc::new(NoDrafts))],
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

async fn exec(db: &Db, sql: &str) {
    sqlx::query(sql).execute(&db.pool).await.unwrap();
}

/// `(id, deleted_at)` for every row of `table`, in id order.
async fn stamps(db: &Db, table: &str) -> Vec<(i64, Option<String>)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr_as(Expr::col(Alias::new("id")), Alias::new("id"))
            .expr_as(
                Expr::col(Alias::new("deleted_at")).cast_as(Alias::new(text_cast(db.backend))),
                Alias::new("deleted_at"),
            )
            .from(Alias::new(table))
            .order_by(Alias::new("id"), Order::Asc)
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.get_int("id").unwrap(),
                r.get_text_opt("deleted_at").unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn delete_trashes_with_the_children_and_restore_brings_back_only_those() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    exec(
        &db,
        "INSERT INTO notes (title) VALUES ('Launch plan'), ('Budget')",
    )
    .await;
    exec(
        &db,
        "INSERT INTO comments (note_id, body, deleted_at) VALUES \
         (1, 'First', NULL), (1, 'Second', NULL), (1, 'Old', '2020-01-01T00:00:00Z')",
    )
    .await;

    let (status, _) = send(&app, "POST", "/admin/notes/delete", &session, "id=1").await;
    assert!(status.is_redirection(), "{status}");

    let notes = stamps(&db, "notes").await;
    let stamp = notes[0].1.clone().expect("the note is trashed");
    assert_eq!(notes[1].1, None);
    let comments = stamps(&db, "comments").await;
    assert_eq!(comments[0].1.as_deref(), Some(stamp.as_str()));
    assert_eq!(comments[1].1.as_deref(), Some(stamp.as_str()));
    assert_eq!(comments[2].1.as_deref(), Some("2020-01-01T00:00:00Z"));

    let (_, list) = send(&app, "GET", "/admin/notes", &session, "").await;
    assert!(
        !list.contains("Launch plan") && list.contains("Budget"),
        "{list}"
    );
    let (_, trash) = send(&app, "GET", "/admin/notes?trashed=1", &session, "").await;
    assert!(
        trash.contains("Launch plan") && !trash.contains("Budget"),
        "{trash}"
    );
    assert!(trash.contains("/admin/notes/restore"), "{trash}");
    let (status, _) = send(&app, "GET", "/admin/notes/1/edit", &session, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(&app, "POST", "/admin/notes/restore", &session, "id=1").await;
    assert!(status.is_redirection(), "{status}");
    assert_eq!(stamps(&db, "notes").await[0].1, None);
    let comments = stamps(&db, "comments").await;
    assert_eq!(comments[0].1, None);
    assert_eq!(comments[1].1, None);
    assert_eq!(
        comments[2].1.as_deref(),
        Some("2020-01-01T00:00:00Z"),
        "a row trashed on its own stays in the trash"
    );
}

#[tokio::test]
async fn delete_permanently_removes_only_trashed_records() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    exec(
        &db,
        "INSERT INTO notes (title) VALUES ('Launch plan'), ('Budget')",
    )
    .await;

    send(&app, "POST", "/admin/notes/purge", &session, "id=2").await;
    assert_eq!(
        stamps(&db, "notes").await.len(),
        2,
        "a live record is not purged"
    );

    send(&app, "POST", "/admin/notes/delete", &session, "id=1").await;
    send(&app, "POST", "/admin/notes/purge", &session, "id=1").await;
    let notes = stamps(&db, "notes").await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].0, 2);
}

#[tokio::test]
async fn a_contributed_hook_shapes_the_list_and_the_form() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    exec(
        &db,
        "INSERT INTO notes (title) VALUES ('Draft: ideas'), ('Budget')",
    )
    .await;
    let (_, list) = send(&app, "GET", "/admin/notes", &session, "").await;
    assert!(
        !list.contains("Draft: ideas") && list.contains("Budget"),
        "{list}"
    );
    let (status, _) = send(&app, "GET", "/admin/notes/1/edit", &session, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
