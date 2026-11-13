//! Polymorphic relations: one comments table and one pivot serve two kinds of
//! parent, each section seeing only its own parent's rows. Imports only the
//! public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::picker::{PickerSourceReg, TableSource};
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
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
        for parent in ["notes", "tasks", "memos"] {
            s.exec(
                Table::create()
                    .table(Alias::new(parent))
                    .col(id())
                    .col(ColumnDef::new(Alias::new("title")).text().not_null())
                    .col(soft_delete_col())
                    .to_owned(),
            )
            .await?;
        }
        s.exec(
            Table::create()
                .table(Alias::new("comments"))
                .col(id())
                .col(
                    ColumnDef::new(Alias::new("commentable_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(key_col(Alias::new("commentable_type")).not_null())
                .col(ColumnDef::new(Alias::new("body")).text().not_null())
                .col(soft_delete_col())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("reactions"))
                .col(id())
                .col(
                    ColumnDef::new(Alias::new("comment_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(key_col(Alias::new("kind")).not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("tags"))
                .col(id())
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("taggables"))
                .col(id())
                .col(
                    ColumnDef::new(Alias::new("taggable_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(key_col(Alias::new("taggable_type")).not_null())
                .col(
                    ColumnDef::new(Alias::new("tag_id"))
                        .big_integer()
                        .not_null(),
                )
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new(
        "acme.morph",
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
            resources: vec![
                resource!("tests/descriptors/morph_notes.yaml"),
                resource!("tests/descriptors/morph_tasks.yaml"),
                resource!("tests/descriptors/morph_comments.yaml"),
                resource!("tests/descriptors/morph_memos.yaml"),
            ],
            permissions: vec![Permission::new("acme.manage", t!("Manage"), t!("Acme"))],
            picker_sources: vec![PickerSourceReg::new(
                "acme.tags",
                Arc::new(TableSource::new("tags", "id", "name")),
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

async fn exec(db: &Db, sql: &str) {
    sqlx::query(sql).execute(&db.pool).await.unwrap();
}

/// `(commentable_type, body, trashed)` for every comment, in id order.
/// The comment each remaining reaction is on.
async fn reactions(db: &Db) -> Vec<i64> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Alias::new("comment_id"))
            .from(Alias::new("reactions"))
            .order_by(Alias::new("id"), Order::Asc)
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get_int("comment_id").unwrap())
        .collect()
}

async fn comments(db: &Db) -> Vec<(String, String, bool)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(["commentable_type", "body", "deleted_at"].map(Alias::new))
            .from(Alias::new("comments"))
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
                r.get_text("commentable_type").unwrap(),
                r.get_text("body").unwrap(),
                r.get_text_opt("deleted_at").unwrap().is_some(),
            )
        })
        .collect()
}

#[tokio::test]
async fn each_parent_sees_and_writes_only_its_own_rows() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    exec(&db, "INSERT INTO notes (title) VALUES ('Launch')").await;
    exec(&db, "INSERT INTO tasks (title) VALUES ('Ship')").await;
    exec(&db, "INSERT INTO tags (name) VALUES ('urgent'), ('later')").await;

    let (status, body) = send(
        &app,
        "POST",
        "/admin/notes/1/comments/new",
        &session,
        "body=On+the+note",
    )
    .await;
    assert!(status.is_redirection(), "{status}: {body}");
    let (status, body) = send(
        &app,
        "POST",
        "/admin/tasks/1/comments/new",
        &session,
        "body=On+the+task",
    )
    .await;
    assert!(status.is_redirection(), "{status}: {body}");
    assert_eq!(
        comments(&db).await,
        [
            ("notes".to_string(), "On the note".to_string(), false),
            ("tasks".to_string(), "On the task".to_string(), false),
        ]
    );

    let (_, note) = send(&app, "GET", "/admin/notes/1/edit", &session, "").await;
    let section = note
        .split("lat-section")
        .skip(1)
        .collect::<Vec<_>>()
        .join("");
    assert!(note.contains("On the note"), "missing own: {section}");
    assert!(!note.contains("On the task"), "has other: {section}");
    // The task's comment shares the id 1 parent but not the type.
    let (status, _) = send(&app, "GET", "/admin/notes/1/comments/2/edit", &session, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(
        &app,
        "POST",
        "/admin/notes/1/tags/new",
        &session,
        "tag_id=1",
    )
    .await;
    assert!(
        status.is_redirection(),
        "attach: {status} {}",
        body.split("lat-field__error")
            .nth(1)
            .unwrap_or(&body[..body.len().min(300)])
    );
    send(
        &app,
        "POST",
        "/admin/tasks/1/tags/new",
        &session,
        "tag_id=2",
    )
    .await;
    let (_, note) = send(&app, "GET", "/admin/notes/1/edit", &session, "").await;
    assert!(
        note.contains("<td>urgent</td>") && !note.contains("<td>later</td>"),
        "{note}"
    );
    let (_, task) = send(&app, "GET", "/admin/tasks/1/edit", &session, "").await;
    assert!(
        task.contains("<td>later</td>") && !task.contains("<td>urgent</td>"),
        "{task}"
    );
}

#[tokio::test]
async fn trashing_a_parent_trashes_only_its_own_kind_of_child() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    exec(&db, "INSERT INTO notes (title) VALUES ('Launch')").await;
    exec(&db, "INSERT INTO tasks (title) VALUES ('Ship')").await;
    exec(
        &db,
        "INSERT INTO comments (commentable_id, commentable_type, body) VALUES \
         (1, 'notes', 'On the note'), (1, 'tasks', 'On the task')",
    )
    .await;
    let (status, _) = send(&app, "POST", "/admin/notes/delete", &session, "id=1").await;
    assert!(status.is_redirection());
    let rows = comments(&db).await;
    assert!(rows[0].2, "the note's comment went with it");
    assert!(!rows[1].2, "the task's comment stayed");
}

#[tokio::test]
async fn a_form_links_the_record_its_own_belongs_to() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    exec(&db, "INSERT INTO notes (title) VALUES ('Launch')").await;
    exec(
        &db,
        "INSERT INTO comments (commentable_id, commentable_type, body) VALUES \
         (1, 'notes', 'On the note'), (9, 'ghosts', 'On nothing shown')",
    )
    .await;

    // The owner is linked by its resource's title and its list's first column.
    let (status, page) = send(&app, "GET", "/admin/comments/1/edit", &session, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains(r#"<span>Written on</span><a href="/admin/notes/1/edit">notes: Launch</a>"#),
        "{page}"
    );
    // An owner no resource shows is named, never linked.
    let (_, page) = send(&app, "GET", "/admin/comments/2/edit", &session, "").await;
    assert!(
        page.contains("<span>Written on</span><b>ghosts #9</b>"),
        "{page}"
    );
    // A form with no linked record shows none.
    let (_, page) = send(&app, "GET", "/admin/notes/1/edit", &session, "").await;
    assert!(!page.contains("lat-linked"), "{page}");
}

#[tokio::test]
async fn deleting_a_parent_deletes_its_own_kind_of_child() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;
    exec(&db, "INSERT INTO notes (title) VALUES ('Launch')").await;
    exec(&db, "INSERT INTO memos (title) VALUES ('Minutes')").await;
    exec(
        &db,
        "INSERT INTO comments (commentable_id, commentable_type, body) VALUES \
         (1, 'notes', 'On the note'), (1, 'memos', 'On the memo')",
    )
    .await;

    exec(
        &db,
        "INSERT INTO reactions (comment_id, kind) VALUES (1, 'like'), (2, 'like'), (2, 'star')",
    )
    .await;

    // The question names what goes with the records, and what goes with that.
    let (_, list) = send(&app, "GET", "/admin/memos", &session, "").await;
    assert!(
        list.contains("These are deleted with them: Comments, Reactions."),
        "{list}"
    );
    let (_, list) = send(&app, "GET", "/admin/notes", &session, "").await;
    assert!(
        list.contains("These go to the trash with them: Comments."),
        "{list}"
    );
    // A record's own page offers the Delete its list does.
    let (_, page) = send(&app, "GET", "/admin/notes/1/edit", &session, "").await;
    assert!(page.contains(r#"action="/admin/notes/delete""#), "{page}");
    assert!(
        page.contains("Move this record to the trash? These go to the trash with it: Comments."),
        "{page}"
    );
    // A relation with no section mounts nothing.
    let (status, _) = send(&app, "GET", "/admin/memos/1/comments", &session, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(&app, "POST", "/admin/memos/delete", &session, "id=1").await;
    assert!(status.is_redirection());
    let rows = comments(&db).await;
    assert_eq!(rows.len(), 1, "the memo's comment went with it");
    assert_eq!(rows[0].0, "notes");
    assert_eq!(reactions(&db).await, [1], "and that comment's reactions");

    // A soft-deleting parent's rows go when it is deleted permanently.
    send(&app, "POST", "/admin/notes/delete", &session, "id=1").await;
    assert_eq!(comments(&db).await.len(), 1, "the trash keeps them");
    let (status, _) = send(&app, "POST", "/admin/notes/purge", &session, "id=1").await;
    assert!(status.is_redirection());
    assert!(comments(&db).await.is_empty());
    assert!(reactions(&db).await.is_empty());
}
