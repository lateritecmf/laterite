//! A resource's writes each asking for their own grant: New, a save and a
//! delete refused without it, the controls hidden, and the form opened
//! read-only. Imports only the public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet};
use sea_query::Alias;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

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
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("title")).text().not_null())
                .to_owned(),
        )
        .await
    }
}

async fn setup() -> (Db, laterite_core::testing::TestGuard, Router) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.posts", vec![Box::new(CreatePosts)]));
    let (db, guard) = laterite_core::testing::connect_test(&sets).await;
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/action_posts.yaml")],
            permissions: vec![
                Permission::new("acme.posts", t!("See posts"), t!("Acme")),
                Permission::new("acme.posts.create", t!("Create posts"), t!("Acme")),
                Permission::new("acme.posts.update", t!("Edit posts"), t!("Acme")),
                Permission::new("acme.posts.delete", t!("Delete posts"), t!("Acme")),
            ],
            ..Default::default()
        },
        config,
        std::sync::Arc::new(CatalogStore::default()),
    );
    let (sql, values) = laterite_core::query::build(
        db.backend,
        sea_query::Query::insert()
            .into_table(Alias::new("posts"))
            .columns([Alias::new("title")])
            .values_panic(["First".into()])
            .to_owned(),
    );
    laterite_core::query::bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await
        .unwrap();
    (db, guard, app)
}

/// A session for an operator holding `grants`.
async fn session(db: &Db, name: &str, grants: &[&str]) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    if grants.is_empty() {
        svc.create_superuser(NewOperator {
            username: name,
            email: &format!("{name}@acme.test"),
            first_name: name,
            last_name: None,
            password: "secretpw12345",
            timezone: None,
        })
        .await
        .unwrap();
    } else {
        let hash = password::hash_password("secretpw12345").unwrap();
        let user = store::create_user(
            db,
            name,
            &format!("{name}@acme.test"),
            name,
            None,
            &hash,
            false,
        )
        .await
        .unwrap();
        let grants: Vec<String> = grants.iter().map(|g| String::from(*g)).collect();
        let role = store::create_role(db, name, name, &grants).await.unwrap();
        store::assign_role(db, user, role).await.unwrap();
    }
    let token = svc
        .authenticate(name, "secretpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    // The token the posts below carry.
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    token
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn get(path: &str, session: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
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
        .header("x-csrf-token", "tok-123")
        .header("x-requested-with", "fetch")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn an_operator_without_a_write_grant_reads_but_cannot_write() {
    let (_db, _guard, app) = setup().await;
    let viewer = session(&_db, "viewer", &["acme.posts"]).await;

    let (status, html) = send(&app, get("/admin/posts", &viewer)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains(r#"href="/admin/posts/new""#), "no New");
    assert!(!html.contains("pick-all"), "no row selection to delete");

    let (status, html) = send(&app, get("/admin/posts/1/edit", &viewer)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"<fieldset class="lat-fieldset" disabled>"#),
        "read-only"
    );
    assert!(!html.contains(r#"type="submit">Save<"#), "no Save");
    assert!(
        !html.contains("Duplicate"),
        "an action needing create is hidden"
    );

    for (status, what) in [
        (
            send(&app, get("/admin/posts/new", &viewer)).await.0,
            "the New form",
        ),
        (
            send(&app, post("/admin/posts/new", &viewer, "title=Two"))
                .await
                .0,
            "a create",
        ),
        (
            send(&app, post("/admin/posts/1/edit", &viewer, "title=Changed"))
                .await
                .0,
            "a save",
        ),
        (
            send(&app, post("/admin/posts/delete", &viewer, "ids=1"))
                .await
                .0,
            "a delete",
        ),
    ] {
        assert_eq!(status, StatusCode::FORBIDDEN, "{what} is refused");
    }
}

#[tokio::test]
async fn each_grant_opens_its_own_write() {
    let (db, _guard, app) = setup().await;
    let editor = session(&db, "editor", &["acme.posts", "acme.posts.update"]).await;
    let (status, html) = send(&app, get("/admin/posts/1/edit", &editor)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains("lat-fieldset"), "editable");
    let (status, _) = send(&app, post("/admin/posts/1/edit", &editor, "title=Changed")).await;
    assert_ne!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&app, get("/admin/posts/new", &editor)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "update does not open create");

    let root = session(&db, "root", &[]).await;
    let (_, html) = send(&app, get("/admin/posts", &root)).await;
    assert!(
        html.contains(r#"href="/admin/posts/new""#),
        "a superuser sees New"
    );
    let (_, html) = send(&app, get("/admin/posts/1/edit", &root)).await;
    assert!(html.contains("Duplicate"));
}
