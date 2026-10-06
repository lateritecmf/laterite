//! The import pages under a resource, end to end: the file in, the columns
//! matched, a check, the run and its report, the refused rows handed back.
//! Imports only the public API.

use std::sync::Arc;
use std::time::Duration;

use laterite_admin::axum::body::Body;
use laterite_admin::axum::http::{header, Request, StatusCode};
use laterite_admin::axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet, ModuleId, Registry};
use laterite_import::find;
use laterite_media::{DiskReg, LocalDisk};
use sea_query::{Alias, Query};
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";
const BOUNDARY: &str = "----laterite";
const IMPORT: &str = "/admin/posts/import";

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

/// The admin with the posts resource (which declares `import: true`), the
/// import module registered, and a private disk in `dir`.
fn app(db: &Db, dir: &std::path::Path) -> Router {
    let mut registry = Registry::new();
    registry.set_owner(ModuleId::new("acme"));
    registry.add(DiskReg::new(
        "private",
        Arc::new(LocalDisk::new(dir.to_path_buf())),
    ));
    registry.set_owner(ModuleId::new("laterite.import"));
    laterite_import::module().register(&mut registry);
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/posts.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
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

/// An upload of `csv` as `posts.csv`, the token first as the guard requires,
/// with the delimiter left for the pages to detect.
fn post_upload(session: &str, csv: &str) -> Request<Body> {
    let mut body = String::new();
    for (name, value) in [("_csrf", CSRF), ("titles", "1"), ("delimiter", "auto")] {
        body.push_str(&format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"posts.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--{BOUNDARY}--\r\n"
    ));
    Request::builder()
        .method("POST")
        .uri(IMPORT)
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

/// Uploads `csv` and returns the matching step's path.
async fn uploaded(app: &Router, session: &str, csv: &str) -> String {
    let (status, location, _) = send(app, post_upload(session, csv)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    location.expect("redirected to the matching step")
}

/// Polls the status fragment until it stops asking to be polled.
async fn settled(app: &Router, session: &str, run: i64) -> String {
    for _ in 0..500 {
        let (status, _, body) =
            send(app, get(&format!("{IMPORT}/runs/{run}/status"), session)).await;
        assert_eq!(status, StatusCode::OK);
        if !body.contains("hx-trigger") {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the run never settled");
}

/// The posts as (code, name, note), in id order.
async fn rows(db: &Db) -> Vec<(String, String, Option<String>)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Alias::new("code"), Alias::new("name"), Alias::new("note")])
            .from(Alias::new("posts"))
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
                row.get_text("code").unwrap(),
                row.get_text("name").unwrap(),
                row.get_text_opt("note").unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_list_offers_import_and_the_pages_serve_a_template() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;

    let (status, _, body) = send(&app, get("/admin/posts", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"href="/admin/posts/import""#),
        "the list offers the import pages"
    );

    let (status, _, body) = send(&app, get(IMPORT, &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No imports yet.") && body.contains("/admin/posts/import/template.csv"));

    let (status, _, body) = send(&app, get(&format!("{IMPORT}/template.csv"), &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body, "\u{feff}code,name,note\r\n",
        "the form's fields as titles"
    );
}

#[tokio::test]
async fn a_check_writes_nothing_and_the_import_follows_it() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;

    let step = uploaded(&app, &session, "code,name\nlaunch,Launch\n,No code\n").await;
    assert!(
        step.starts_with(&format!("{IMPORT}/1?titles=1&delimiter=auto")),
        "{step}"
    );
    let (status, _, body) = send(&app, get(&step, &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("posts.csv") && body.contains("Comma"),
        "the file and its detected delimiter"
    );
    assert!(
        body.contains(r#"<option value="code" selected>"#)
            && body.contains(r#"<option value="name" selected>"#)
    );

    // The check runs every row with nothing written.
    let (status, location, _) = send(
        &app,
        post_form(
            &format!("{IMPORT}/1"),
            &session,
            "match_0=code&match_1=name&key=&on_match=skip&keep_blank=1&titles=1&delimiter=auto&action=check",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some(&format!("{IMPORT}/runs/1")[..]));
    let fragment = settled(&app, &session, 1).await;
    assert!(fragment.contains("nothing was written") && fragment.contains("Would create"));
    assert!(
        fragment.contains(&format!("{IMPORT}/runs/1/import")),
        "the settled fragment offers the real run"
    );
    assert!(
        fragment.contains(&format!("{IMPORT}/runs/1/refused.csv")),
        "and the refused rows"
    );
    assert!(rows(&db).await.is_empty(), "a check stores nothing");
    let check = find(&db, 1).await.unwrap().unwrap();
    assert_eq!((check.created, check.errors), (1, 1));
    let (_, _, page) = send(&app, get(&format!("{IMPORT}/runs/1"), &session)).await;
    assert!(
        page.contains(&format!("{IMPORT}/runs/1/import")),
        "the page carries the fragment"
    );

    // The real run uses the matching the check used.
    let (status, location, _) = send(
        &app,
        post_form(&format!("{IMPORT}/runs/1/import"), &session, ""),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some(&format!("{IMPORT}/runs/2")[..]));
    let fragment = settled(&app, &session, 2).await;
    assert!(fragment.contains("Done") && !fragment.contains("nothing was written"));
    assert_eq!(
        rows(&db).await,
        [("launch".to_string(), "Launch".to_string(), None)]
    );
    let (_, _, page) = send(&app, get(&format!("{IMPORT}/runs/2"), &session)).await;
    assert!(
        page.contains("<td>3</td><td>Refused</td><td>code</td>"),
        "the refused row, by its line"
    );

    // The refused rows come back as a file, with the problem beside each.
    let (status, _, body) =
        send(&app, get(&format!("{IMPORT}/runs/2/refused.csv"), &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("\u{feff}code,name,problem"), "{body}");
    assert!(
        body.contains(",No code,code: ") && !body.contains("launch"),
        "only the refused row"
    );

    // The index lists both, by kind.
    let (_, _, body) = send(&app, get(IMPORT, &session)).await;
    assert!(body.contains("<td>Check</td>") && body.contains("<td>Import</td>"));
}

#[tokio::test]
async fn a_file_with_the_same_titles_is_matched_as_before() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;

    // Titles nothing matches by name, matched by hand once.
    let step = uploaded(&app, &session, "ref,title\nlaunch,Launch\n").await;
    let (_, _, body) = send(&app, get(&step, &session)).await;
    assert!(
        !body.contains(r#"<option value="code" selected>"#)
            && !body.contains(r#"<option value="name" selected>"#),
        "nothing is proposed for unfamiliar titles"
    );
    let (status, _, _) = send(
        &app,
        post_form(
            &format!("{IMPORT}/1"),
            &session,
            "match_0=code&match_1=name&key=code&on_match=update&titles=1&delimiter=auto&action=import",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    settled(&app, &session, 1).await;

    // The next file with those titles is proposed the same way.
    let step = uploaded(&app, &session, "ref,title\nrelaunch,Relaunch\n").await;
    let (_, _, body) = send(&app, get(&step, &session)).await;
    assert!(body.contains("Matched the way your last file"));
    assert!(
        body.contains(r#"<option value="code" selected>"#)
            && body.contains(r#"<option value="name" selected>"#)
    );
    assert!(
        body.contains(r#"value="update" checked"#),
        "and the duplicates policy with it"
    );
}

#[tokio::test]
async fn semicolons_are_detected_and_a_blank_cell_keeps_or_clears() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;
    let (status, _, _) = send(
        &app,
        post_form(
            "/admin/posts/new",
            &session,
            "code=launch&name=Launch&note=kept",
        ),
    )
    .await;
    assert!(
        status.is_redirection() || status == StatusCode::NO_CONTENT,
        "{status}"
    );

    let step = uploaded(&app, &session, "code;name;note\nlaunch;Relaunch;\n").await;
    let (_, _, body) = send(&app, get(&step, &session)).await;
    assert!(
        body.contains("Semicolon"),
        "the delimiter is read off the file"
    );
    let update = |keep: &str| {
        format!("match_0=code&match_1=name&match_2=note&key=code&on_match=update{keep}&titles=1&delimiter=auto&action=import")
    };

    send(
        &app,
        post_form(&format!("{IMPORT}/1"), &session, &update("&keep_blank=1")),
    )
    .await;
    settled(&app, &session, 1).await;
    assert_eq!(
        rows(&db).await,
        [(
            "launch".to_string(),
            "Relaunch".to_string(),
            Some("kept".to_string())
        )]
    );

    send(
        &app,
        post_form(&format!("{IMPORT}/1"), &session, &update("")),
    )
    .await;
    settled(&app, &session, 2).await;
    assert_eq!(
        rows(&db).await,
        [("launch".to_string(), "Relaunch".to_string(), None)]
    );
}

#[tokio::test]
async fn the_matching_step_refuses_what_cannot_run() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;
    let step = uploaded(&app, &session, "code,name\nlaunch,Launch\n").await;
    let path = step.split('?').next().unwrap().to_string();

    let (status, _, body) = send(
        &app,
        post_form(&path, &session, "match_0=&match_1=&titles=1&action=check"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Match at least one column to a field."));
    let (_, _, body) = send(
        &app,
        post_form(&path, &session, "match_0=code&titles=1&action=check"),
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
            "match_0=code&match_1=name&key=note&on_match=skip&titles=1&action=check",
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
async fn a_missing_file_or_run_is_not_found_and_an_empty_upload_is_sent_back() {
    let (db, _guard) = test_db().await;
    let dir = tempfile::tempdir().unwrap();
    let app = app(&db, dir.path());
    let session = session(&db).await;

    let (status, _, _) = send(&app, get(&format!("{IMPORT}/99"), &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&app, get(&format!("{IMPORT}/runs/99"), &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, location, _) = send(&app, post_upload(&session, "")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some(IMPORT));
    let (_, _, body) = send(&app, get(IMPORT, &session)).await;
    assert!(
        body.contains("Choose a file to import."),
        "the flash says why"
    );
}
