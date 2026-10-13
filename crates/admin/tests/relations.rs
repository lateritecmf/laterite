//! Related records on a resource's edit page: the section, the child pages
//! under the parent, the foreign key the framework owns. Imports only the
//! public API.

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

struct CreateTables;

#[async_trait(?Send)]
impl Migration for CreateTables {
    fn name(&self) -> &str {
        "0001_create_posts_and_comments"
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
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("comments"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(Alias::new("post_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(Alias::new("body")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("tags"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(Alias::new("post_tags"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(Alias::new("post_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(Alias::new("tag_id"))
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(Alias::new("note")).text().null())
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new(
        "acme.posts",
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
            resources: vec![resource!("tests/descriptors/relations.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
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

fn get(path: &str, session: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap()
}

fn get_htmx(path: &str, session: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("hx-request", "true")
        .body(Body::empty())
        .unwrap()
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

/// The comments as (post_id, body), in id order.
async fn comments(db: &Db) -> Vec<(i64, String)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Alias::new("post_id"), Alias::new("body")])
            .from(Alias::new("comments"))
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
                row.get_int("post_id").unwrap(),
                row.get_text("body").unwrap(),
            )
        })
        .collect()
}

/// The memberships as (post_id, tag_id, note), in id order.
async fn memberships(db: &Db) -> Vec<(i64, i64, String)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                Alias::new("post_id"),
                Alias::new("tag_id"),
                Alias::new("note"),
            ])
            .from(Alias::new("post_tags"))
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
                row.get_int("post_id").unwrap(),
                row.get_int("tag_id").unwrap(),
                row.get_text("note").unwrap_or_default(),
            )
        })
        .collect()
}

/// Two tags, written straight to the table: they are not the resource under test.
async fn two_tags(db: &Db) {
    for name in ["rust", "news"] {
        let (sql, values) = build(
            db.backend,
            Query::insert()
                .into_table(Alias::new("tags"))
                .columns([Alias::new("name")])
                .values_panic([name.into()])
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}

/// Two posts, created through the form; returns the session.
async fn two_posts(app: &Router, db: &Db) -> String {
    let session = session(db).await;
    for (code, name) in [("launch", "Launch"), ("relaunch", "Relaunch")] {
        let (status, _, _) = send(
            app,
            post(
                "/admin/posts/new",
                &session,
                &format!("code={code}&name={name}"),
            ),
        )
        .await;
        assert!(status.is_redirection(), "{status}");
    }
    session
}

#[tokio::test]
async fn the_edit_page_carries_a_section_and_the_create_page_names_it() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = two_posts(&app, &db).await;

    let (status, _, body) = send(&app, get("/admin/posts/new", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Save first, then add") && body.contains("Comments"),
        "{body}"
    );
    assert!(
        body.contains(r#"name="then" value="stay""#),
        "save and continue is offered"
    );

    let (status, _, body) = send(&app, get("/admin/posts/1/edit", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h2>Comments</h2>"), "the section heading");
    assert!(
        body.contains(r#"href="/admin/posts/1/comments/new""#),
        "a New link under the parent"
    );
    assert!(
        body.contains(r#"id="lat-rel-comments""#),
        "the section's own region"
    );
    assert!(
        !body.contains("Save first"),
        "nothing is pending on an edit page"
    );
}

#[tokio::test]
async fn a_related_record_is_written_under_the_url_parent_not_the_submitted_one() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = two_posts(&app, &db).await;

    let (status, _, body) = send(&app, get("/admin/posts/1/comments/new", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"<input type="hidden" name="post_id""#),
        "the foreign key is a hidden input"
    );
    assert!(
        !body.contains(r#"for="post_id""#),
        "and never a labelled field"
    );

    // The submission claims post 2; the URL says post 1, and the URL wins.
    let (status, location, _) = send(
        &app,
        post(
            "/admin/posts/1/comments/new",
            &session,
            "post_id=2&body=First",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        location.as_deref(),
        Some("/admin/posts/1/edit"),
        "a save returns to the parent"
    );
    assert_eq!(comments(&db).await, [(1, "First".to_string())]);

    // The section lists it, and only under its own parent.
    let (_, _, body) = send(&app, get("/admin/posts/1/edit", &session)).await;
    assert!(body.contains("First") && body.contains(r#"href="/admin/posts/1/comments/1/edit""#));
    let (_, _, body) = send(&app, get("/admin/posts/2/edit", &session)).await;
    assert!(!body.contains("First"), "post 2 has no comments");
    let (status, _, _) = send(&app, get("/admin/posts/2/comments/1/edit", &session)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a child under the wrong parent is not found"
    );
    let (status, _, _) = send(&app, get("/admin/posts/9/comments/new", &session)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no parent, no child form");
}

#[tokio::test]
async fn a_related_record_is_edited_and_deleted_under_its_parent() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = two_posts(&app, &db).await;
    send(
        &app,
        post("/admin/posts/1/comments/new", &session, "body=First"),
    )
    .await;
    send(
        &app,
        post("/admin/posts/1/comments/new", &session, "body=Second"),
    )
    .await;

    let (status, location, _) = send(
        &app,
        post(
            "/admin/posts/1/comments/1/edit",
            &session,
            "post_id=2&body=First%2C%20edited",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/edit"));
    assert_eq!(
        comments(&db).await,
        [(1, "First, edited".to_string()), (1, "Second".to_string())],
        "the foreign key stays with the URL's parent"
    );

    // The section's region sorts and pages on its own path.
    let (status, _, body) = send(
        &app,
        get_htmx("/admin/posts/1/comments?sort=body&dir=desc", &session),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with(r#"<div id="lat-rel-comments""#), "{body}");
    assert!(
        body.find("Second").unwrap() < body.find("First, edited").unwrap(),
        "sorted as asked"
    );
    assert!(
        body.contains(r#"hx-get="/admin/posts/1/comments?sort="#),
        "links carry the section's path"
    );
    let (status, _, body) = send(&app, get("/admin/posts/1/comments", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Comments") && body.contains("lat-topnav"),
        "without scripting, a page of its own"
    );

    // A per-row delete under the parent.
    let (status, location, _) = send(
        &app,
        post("/admin/posts/1/comments/delete", &session, "id=2"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/edit"));
    assert_eq!(comments(&db).await, [(1, "First, edited".to_string())]);
    // A delete naming another parent's record does nothing.
    let (status, _, _) = send(
        &app,
        post("/admin/posts/2/comments/delete", &session, "id=1"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(comments(&db).await.len(), 1);
}

#[tokio::test]
async fn save_and_continue_lands_on_the_edit_page() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = session(&db).await;

    let (status, location, _) = send(
        &app,
        post(
            "/admin/posts/new",
            &session,
            "code=launch&name=Launch&then=stay",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/edit"));
    let (status, location, _) = send(
        &app,
        post(
            "/admin/posts/1/edit",
            &session,
            "code=launch&name=Launch%20again&then=stay",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/edit"));
    let (_, location, _) = send(
        &app,
        post("/admin/posts/1/edit", &session, "code=launch&name=Launch"),
    )
    .await;
    assert_eq!(
        location.as_deref(),
        Some("/admin/posts"),
        "a plain save still returns to the list"
    );
}

#[tokio::test]
async fn a_record_is_attached_through_the_picker_and_listed_only_under_its_parent() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = two_posts(&app, &db).await;
    two_tags(&db).await;

    let (status, _, body) = send(&app, get("/admin/posts/1/edit", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h2>Tags</h2>"), "the section heading");
    assert!(
        body.contains(r#"href="/admin/posts/1/tags/new""#) && body.contains("Attach Tag"),
        "an Attach link under the parent: {body}"
    );

    let (status, _, body) = send(&app, get("/admin/posts/1/tags/new", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"data-lat-widget="ref-picker""#) && body.contains("acme.tags"),
        "the record is chosen in the picker over the declared source"
    );
    assert!(
        body.contains(r#"<input type="hidden" name="post_id""#),
        "the parent key is hidden"
    );
    assert!(
        body.contains(r#"for="note""#),
        "the pivot field is asked for"
    );

    // The submission claims post 2; the URL says post 1, and the URL wins.
    let (status, location, _) = send(
        &app,
        post(
            "/admin/posts/1/tags/new",
            &session,
            "post_id=2&tag_id=2&note=lead",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/edit"));
    assert_eq!(memberships(&db).await, [(1, 2, "lead".to_string())]);

    // The section lists the tag by its own columns, keyed by the membership.
    let (_, _, body) = send(&app, get("/admin/posts/1/edit", &session)).await;
    assert!(
        body.contains("news") && body.contains(r#"href="/admin/posts/1/tags/1/edit""#),
        "{body}"
    );
    assert!(
        body.contains(">Remove<"),
        "a membership is removed, not deleted"
    );
    let (_, _, body) = send(&app, get("/admin/posts/2/edit", &session)).await;
    assert!(!body.contains("news"), "post 2 has no tags");

    // Attaching it again is refused; attaching nothing is the form's refusal.
    let (status, location, _) =
        send(&app, post("/admin/posts/1/tags/new", &session, "tag_id=2")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/tags/new"));
    let (status, _, body) = send(&app, get("/admin/posts/1/tags/new", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("already attached"), "{body}");
    let (status, _, _) = send(&app, post("/admin/posts/1/tags/new", &session, "tag_id=9")).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "no such record");
    let (status, _, body) = send(&app, post("/admin/posts/1/tags/new", &session, "tag_id=")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(memberships(&db).await.len(), 1);
}

#[tokio::test]
async fn a_membership_is_edited_and_removed_under_its_parent_leaving_the_record() {
    let (db, _guard) = test_db().await;
    let app = app(&db);
    let session = two_posts(&app, &db).await;
    two_tags(&db).await;
    send(
        &app,
        post("/admin/posts/1/tags/new", &session, "tag_id=1&note=first"),
    )
    .await;

    let (status, _, body) = send(&app, get("/admin/posts/1/tags/1/edit", &session)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#"value="first""#), "the pivot row's values");
    assert!(
        !body.contains(r#"for="tag_id""#),
        "the membership itself is fixed"
    );
    let (status, _, _) = send(&app, get("/admin/posts/2/tags/1/edit", &session)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a membership under the wrong parent"
    );

    // The submission tries to move the membership to tag 2; the row's tag wins.
    let (status, location, _) = send(
        &app,
        post(
            "/admin/posts/1/tags/1/edit",
            &session,
            "tag_id=2&post_id=2&note=second",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/edit"));
    assert_eq!(memberships(&db).await, [(1, 1, "second".to_string())]);

    let (status, _, body) = send(&app, post("/admin/posts/2/tags/delete", &session, "id=1")).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        memberships(&db).await.len(),
        1,
        "not under the wrong parent"
    );
    let (status, location, _) =
        send(&app, post("/admin/posts/1/tags/delete", &session, "id=1")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location.as_deref(), Some("/admin/posts/1/edit"));
    assert!(memberships(&db).await.is_empty());
    let (_, _, body) = send(&app, get("/admin/posts/1/edit", &session)).await;
    assert!(body.contains("Removed 1 record."), "{body}");

    // The tag itself is untouched: it can be attached again.
    let (status, _, body) = send(&app, post("/admin/posts/1/tags/new", &session, "tag_id=1")).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(memberships(&db).await, [(1, 1, String::new())]);
}
