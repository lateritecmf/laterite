//! A resource with a `source` keeps its records as files: the folder is
//! read into the table at boot, a save writes the file back, a delete
//! removes it, and with `kind = "git"` every write is a commit. Imports
//! only the framework's public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::{FormConfig, FormField};
use laterite_admin::list::{ListColumn, ListConfig};
use laterite_admin::{router, AdminConfig, Contributions, Permission, Resource};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::source::{Format, Source, FILES, GIT};
use laterite_core::strata::*;
use laterite_core::{t, AnyRowExt, CatalogStore, Db, ModelListenerReg};
use sea_query::Alias;
use std::path::Path;
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

struct Posts;

#[async_trait(?Send)]
impl laterite_core::Migration for Posts {
    fn name(&self) -> &str {
        "0001_create_posts"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("posts"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("slug")).text().not_null())
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .col(
                    laterite_core::bool_col(Alias::new("pinned"))
                        .not_null()
                        .default(0),
                )
                .col(ColumnDef::new(Alias::new("body")).text().null())
                .col(ColumnDef::new(Alias::new("created_at")).text().not_null())
                .col(ColumnDef::new(Alias::new("updated_at")).text().not_null())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(MigrationSet::new("acme", vec![Box::new(Posts)]));
    laterite_core::testing::connect_test(&migrations).await
}

fn source(kind: &str) -> Source {
    let mut source = Source::files("content/posts", Format::Markdown)
        .body("body")
        .slug_from("title");
    source.kind = kind.to_string();
    source
}

fn resource(kind: &str) -> Resource {
    Resource::new(
        "/posts",
        "Posts",
        ListConfig::new("posts", "Posts", vec![ListColumn::new("title", "Title")]).deletable(),
    )
    .form(
        FormConfig::new(
            "posts",
            "Post",
            "/posts",
            "id",
            vec![
                FormField::text("slug", "Slug"),
                FormField::text("title", "Title").required(),
                FormField::switch("pinned", "Pinned"),
                FormField::textarea("body", "Body"),
            ],
        )
        .timestamps(),
    )
    .permission("acme.manage_posts")
    .source(source(kind))
}

/// Boots the resource as the framework does: the folder is read into the
/// table and the listener comes back for the router.
async fn boot(db: &Db, root: &Path, kind: &str) -> Vec<ModelListenerReg> {
    laterite_admin::source::sync(db, root, &[resource(kind)], &[], &[])
        .await
        .unwrap()
}

fn app(db: &Db, kind: &str, listeners: Vec<ModelListenerReg>) -> Router {
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource(kind)],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            listeners,
            ..Default::default()
        },
        AdminConfig::default(),
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
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    token
}

async fn post(app: &Router, path: &str, session: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn rows(db: &Db) -> Vec<(i64, String, String, Option<String>)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                Alias::new("id"),
                Alias::new("slug"),
                Alias::new("title"),
                Alias::new("body"),
            ])
            .from(Alias::new("posts"))
            .order_by(Alias::new("slug"), Order::Asc)
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get_int("id").unwrap(),
                row.get_text("slug").unwrap(),
                row.get_text("title").unwrap(),
                row.get_text_opt("body").unwrap(),
            )
        })
        .collect()
}

fn failure<T>(result: anyhow::Result<T>) -> String {
    match result {
        Ok(_) => panic!("expected a failure"),
        Err(e) => e.to_string(),
    }
}

fn write(root: &Path, name: &str, text: &str) {
    let dir = root.join("content/posts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), text).unwrap();
}

fn read(root: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(root.join("content/posts").join(name)).ok()
}

#[tokio::test]
async fn boot_reads_the_folder_into_the_table_and_the_files_stay_the_truth() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "hello.md",
        "---\ntitle: Hello\npinned: true\n---\n# Hi\n\nFirst post.\n",
    );
    write(root, "second.md", "---\ntitle: Second\n---\nAnother.\n");

    boot(&db, root, FILES).await;
    let pulled = rows(&db).await;
    assert_eq!(pulled.len(), 2);
    assert_eq!(pulled[0].1, "hello");
    assert_eq!(pulled[0].2, "Hello");
    assert_eq!(pulled[0].3.as_deref(), Some("# Hi\n\nFirst post."));
    assert_eq!(pulled[1].1, "second");

    // A second boot with one file gone and one changed: the table follows.
    std::fs::remove_file(root.join("content/posts/second.md")).unwrap();
    write(
        root,
        "hello.md",
        "---\ntitle: Hello again\n---\nFirst post.\n",
    );
    boot(&db, root, FILES).await;
    let pulled = rows(&db).await;
    assert_eq!(pulled.len(), 1);
    assert_eq!(pulled[0].2, "Hello again");
}

#[tokio::test]
async fn a_file_with_an_unknown_field_or_a_bad_declaration_names_itself() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "odd.md", "---\ntitle: Odd\ncolour: red\n---\n");
    let err = failure(laterite_admin::source::sync(&db, root, &[resource(FILES)], &[], &[]).await);
    assert!(err.contains("odd.md") && err.contains("colour"), "{err}");

    let mut bad = source(FILES);
    bad.key = "missing".into();
    let bad_key = Resource::new(
        "/posts",
        "Posts",
        ListConfig::new("posts", "Posts", vec![ListColumn::new("title", "Title")]),
    )
    .form(FormConfig::new(
        "posts",
        "Post",
        "/posts",
        "id",
        vec![FormField::text("title", "Title")],
    ))
    .permission("acme.manage_posts")
    .source(bad);
    let err = failure(laterite_admin::source::sync(&db, root, &[bad_key], &[], &[]).await);
    assert!(err.contains("source.key"), "{err}");

    let unknown = resource("acme.sheets");
    let err = failure(laterite_admin::source::sync(&db, root, &[unknown], &[], &[]).await);
    assert!(err.contains("acme.sheets"), "{err}");
}

#[tokio::test]
async fn a_save_writes_the_file_and_a_delete_removes_it() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listeners = boot(&db, root, FILES).await;
    let app = app(&db, FILES, listeners);
    let operator = session(&db).await;

    // A create with a blank slug derives it from the title, and the file
    // carries the form's columns in its order, the switch as a boolean.
    assert_eq!(
        post(
            &app,
            "/admin/posts/new",
            &operator,
            "slug=&title=Friday+Night%2C+Live%21&pinned=1&body=See+you+there."
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let saved = rows(&db).await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].1, "friday-night-live");
    assert_eq!(
        read(root, "friday-night-live.md").as_deref(),
        Some("---\ntitle: Friday Night, Live!\npinned: true\n---\nSee you there.\n")
    );

    // A key the file system cannot take is refused against the field.
    assert_eq!(
        post(
            &app,
            "/admin/posts/new",
            &operator,
            "slug=bad%2Fslug&title=Nope&body="
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(rows(&db).await.len(), 1);

    // Renaming the key moves the file.
    let id = saved[0].0;
    assert_eq!(
        post(
            &app,
            &format!("/admin/posts/{id}/edit"),
            &operator,
            "slug=friday&title=Friday&body=Moved."
        )
        .await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(read(root, "friday-night-live.md"), None);
    assert_eq!(
        read(root, "friday.md").as_deref(),
        Some("---\ntitle: Friday\npinned: false\n---\nMoved.\n")
    );

    // A delete removes the file.
    assert_eq!(
        post(&app, "/admin/posts/delete", &operator, &format!("id={id}")).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(read(root, "friday.md"), None);
    assert!(rows(&db).await.is_empty());
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[tokio::test]
async fn with_the_git_kind_every_write_is_a_commit_naming_the_operator() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "ci@acme.test"]);
    git(root, &["config", "user.name", "CI"]);
    git(root, &["config", "commit.gpgsign", "false"]);

    let listeners = boot(&db, root, GIT).await;
    let app = app(&db, GIT, listeners);
    let operator = session(&db).await;
    assert_eq!(
        post(
            &app,
            "/admin/posts/new",
            &operator,
            "slug=hello&title=Hello&body=Hi"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let log = git(root, &["log", "--format=%s"]);
    assert_eq!(log.trim(), "Added content/posts/hello.md (root)");
    assert!(git(root, &["status", "--porcelain"]).trim().is_empty());

    let id = rows(&db).await[0].0;
    assert_eq!(
        post(&app, "/admin/posts/delete", &operator, &format!("id={id}")).await,
        StatusCode::SEE_OTHER
    );
    let log = git(root, &["log", "--format=%s"]);
    assert_eq!(
        log.lines().next(),
        Some("Removed content/posts/hello.md (root)")
    );
}
