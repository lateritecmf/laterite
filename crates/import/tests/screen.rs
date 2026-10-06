//! The import screen, end to end: the file in, the columns matched, the run
//! started and its report read. Imports only the public API.

use std::sync::Arc;
use std::time::Duration;

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{header, Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::routes::ScreenReg;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet, ModuleId, Registry};
use laterite_import::{find, ImportReg, Status};
use laterite_media::{DiskReg, LocalDisk};
use sea_query::Alias;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";
const BOUNDARY: &str = "----laterite";

struct CreatePosts;

#[async_trait(?Send)]
impl Migration for CreatePosts {
    fn name(&self) -> &str {
        "0001_create_posts"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("posts"))
                .if_not_exists()
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("note")).text())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(laterite_media::module().migrations());
    sets.push(laterite_import::module().migrations());
    sets.push(MigrationSet::new("acme.posts", vec![Box::new(CreatePosts)]));
    laterite_core::testing::connect_test(&sets).await
}

/// The admin with the posts resource, the import module's screen and
/// permission, a private disk in `dir`, and posts registered for import.
fn app(db: &Db, dir: &std::path::Path) -> Router {
    let mut registry = Registry::new();
    registry.set_owner(ModuleId::new("acme"));
    registry.add(ImportReg::new("/posts", "Posts"));
    registry.add(DiskReg::new(
        "private",
        Arc::new(LocalDisk::new(dir.to_path_buf())),
    ));
    registry.set_owner(ModuleId::new("laterite.import"));
    laterite_import::module().register(&mut registry);
    let screens: Vec<ScreenReg> = registry
        .take_owned::<ScreenReg>()
        .into_iter()
        .map(|(_, mut reg)| {
            // The boot would mount it under the module's base; here it is named.
            reg.base_path = "/import".to_string();
            reg
        })
        .collect();
    let mut permissions: Vec<Permission> = registry
        .take_owned::<Permission>()
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    permissions.push(Permission::new(
        "acme.manage_posts",
        t!("Manage posts"),
        t!("Acme"),
    ));
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/posts.yaml")],
            permissions,
            screens,
            plugin_defined: Arc::new(registry),
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

fn post_form(path: &str, session: &str, body: &str) -> Request<Body> {
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

/// An upload of `csv` as `posts.csv`, the token first as the guard requires.
fn post_upload(path: &str, session: &str, csv: &str) -> Request<Body> {
    let mut body = String::new();
    for (name, value) in [("_csrf", CSRF), ("titles", "1"), ("delimiter", ",")] {
        body.push_str(&format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"posts.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--{BOUNDARY}--\r\n"
    ));
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("sec-fetch-site", "same-origin")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
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
    let bytes = laterite_admin::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&bytes).to_string(),
    )
}

/// Polls the status fragment until it stops asking to be polled.
async fn settled(app: &Router, session: &str, run: i64) -> String {
    for _ in 0..500 {
        let (status, _, body) = send(
            app,
            get(&format!("/admin/import/runs/{run}/status"), session),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        if !body.contains("hx-trigger") {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the run never settled");
}

#[tokio::test]
async fn a_file_goes_in_and_its_report_comes_out() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;

    // The index offers the registered resource and no runs yet.
    let (status, _, body) = send(&app, get("/admin/import", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Posts"), "the resource is offered");
    assert!(body.contains("/admin/import/upload/posts"));
    assert!(body.contains("No runs yet."));

    // The upload lands on the private disk and leads to the matching step.
    let (status, location, _) = send(
        &app,
        post_upload(
            "/admin/import/upload/posts",
            &session,
            "code,name\nlaunch,Launch\n,No code\n",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = location.expect("redirected to the matching step");
    assert!(
        location.starts_with("/admin/import/match/1/posts?titles=1&delimiter=%2C"),
        "{location}"
    );
    assert!(dir.path().join("blobs").is_dir(), "the file was stored");

    // The matching step shows the file's columns with the form's fields proposed.
    let (status, _, body) = send(&app, get(&location, &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("posts.csv"));
    assert!(body.contains(r#"name="match_0""#) && body.contains(r#"name="match_1""#));
    assert!(
        body.contains(r#"<option value="code" selected>"#),
        "code proposed for its column"
    );
    assert!(
        body.contains(r#"<option value="name" selected>"#),
        "name proposed for its column"
    );
    assert!(body.contains("Launch, No code"), "the samples are shown");

    // Starting the run redirects to its report.
    let (status, location, _) = send(
        &app,
        post_form(
            "/admin/import/match/1/posts",
            &session,
            "match_0=code&match_1=name&key=&on_match=skip&titles=1&delimiter=%2C",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/import/runs/1"));

    // The report polls until the run settles, then shows the counts and notes.
    let settled = settled(&app, &session, 1).await;
    assert!(settled.contains("Done"));
    let (status, _, page) = send(&app, get("/admin/import/runs/1", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains("Posts") && page.contains("root"),
        "what ran, and who"
    );
    assert!(
        page.contains("<td>3</td><td>Refused</td><td>code</td>"),
        "the refused row, by its line"
    );
    let run = find(&db, 1).await.unwrap().unwrap();
    assert_eq!(run.status, Status::Done);
    assert_eq!((run.created, run.errors), (1, 1));

    // The index now lists it.
    let (_, _, body) = send(&app, get("/admin/import", &session)).await;
    assert!(body.contains("/admin/import/runs/1") && !body.contains("No runs yet."));
}

#[tokio::test]
async fn the_matching_step_refuses_what_cannot_run() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;
    let (_, location, _) = send(
        &app,
        post_upload(
            "/admin/import/upload/posts",
            &session,
            "code,name\nlaunch,Launch\n",
        ),
    )
    .await;
    let path = location.unwrap();
    let path = path.split('?').next().unwrap().to_string();

    let (status, _, body) = send(
        &app,
        post_form(&path, &session, "match_0=&match_1=&titles=1&delimiter=%2C"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Match at least one column to a field."));

    let (_, _, body) = send(
        &app,
        post_form(&path, &session, "match_0=code&titles=1&delimiter=%2C"),
    )
    .await;
    assert!(
        body.contains("must be matched to a column."),
        "name is required by the form"
    );

    let (_, _, body) = send(
        &app,
        post_form(
            &path,
            &session,
            "match_0=code&match_1=name&key=note&on_match=skip&titles=1&delimiter=%2C",
        ),
    )
    .await;
    assert!(
        body.contains("must itself be matched"),
        "the key has to come from the file"
    );
    assert!(find(&db, 1).await.unwrap().is_none(), "nothing started");
}

#[tokio::test]
async fn an_unregistered_resource_and_a_missing_file_are_not_found() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;

    let (status, _, _) = send(&app, get("/admin/import/upload/nope", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, get("/admin/import/match/99/posts", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, get("/admin/import/runs/99", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_upload_without_a_file_is_sent_back() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;

    let (status, location, _) = send(
        &app,
        post_upload("/admin/import/upload/posts", &session, ""),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/import/upload/posts"));
    let (_, _, body) = send(&app, get("/admin/import/upload/posts", &session)).await;
    assert!(
        body.contains("Choose a file to import."),
        "the flash says why"
    );
}
