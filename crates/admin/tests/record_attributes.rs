//! Footprints, generated slugs and encrypted columns, declared on a form and
//! applied on every save. Imports only the public API.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, AnyRowExt, CatalogStore, Crypt, Db, MigrationSet, Record};
use sea_query::{Alias, Query};
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";
const ORIGIN: &str = "https://acme.test";
const CSRF: &str = "tok-123";

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
                .col(ColumnDef::new(Alias::new("slug")).text().null())
                .col(ColumnDef::new(Alias::new("api_key")).text().null())
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
                .to_owned(),
        )
        .await
    }
}

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.posts", vec![Box::new(CreatePosts)]));
    laterite_core::testing::connect_test(&sets).await
}

fn app(db: &Db, crypt: Arc<Crypt>) -> Router {
    let mut config = AdminConfig::default();
    config.origin = ORIGIN.to_string();
    config.crypt = Some(crypt);
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/attributed_posts.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    )
}

/// A superuser's session and their id.
async fn session(db: &Db) -> (String, i64) {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    let user_id = svc
        .create_superuser(NewOperator {
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
    (token, user_id)
}

async fn post(app: &Router, path: &str, session: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", CSRF)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Every post as a record, in id order.
async fn posts(db: &Db) -> Vec<Record> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(
                [
                    "id",
                    "title",
                    "slug",
                    "api_key",
                    "created_user_id",
                    "updated_user_id",
                ]
                .map(Alias::new),
            )
            .from(Alias::new("posts"))
            .order_by(Alias::new("id"), sea_query::Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .unwrap();
    rows.iter()
        .map(|r| {
            let mut rec = Record::with_id("posts", r.get_int("id").unwrap());
            for column in ["title", "slug", "api_key"] {
                if let Ok(Some(value)) = r.get_text_opt(column) {
                    rec.set(column, value);
                }
            }
            for column in ["created_user_id", "updated_user_id"] {
                rec.set(column, r.get_int_opt(column).unwrap());
            }
            rec
        })
        .collect()
}

#[tokio::test]
async fn a_save_stamps_the_user_fills_a_unique_slug_and_seals_the_column() {
    let (db, _guard) = test_db().await;
    let crypt = Arc::new(Crypt::ephemeral());
    let app = app(&db, crypt.clone());
    let (session, user_id) = session(&db).await;

    for _ in 0..3 {
        let (status, body) = post(
            &app,
            "/admin/posts/new",
            &session,
            "title=Hello+World&slug=&api_key=sk-live-123",
        )
        .await;
        assert!(status.is_redirection(), "{status}: {body}");
    }

    let rows = posts(&db).await;
    let slugs: Vec<_> = rows.iter().map(|r| r.text("slug").unwrap()).collect();
    assert_eq!(slugs, ["hello-world", "hello-world-2", "hello-world-3"]);
    for row in &rows {
        assert_eq!(row.int("created_user_id"), Some(user_id));
        assert_eq!(row.int("updated_user_id"), Some(user_id));
        assert_ne!(row.text("api_key"), Some("sk-live-123"), "stored sealed");
        assert_eq!(
            row.opened(&crypt, "api_key").as_deref(),
            Some("sk-live-123")
        );
    }
}

#[tokio::test]
async fn a_typed_slug_another_post_holds_is_refused_on_its_field() {
    let (db, _guard) = test_db().await;
    let app = app(&db, Arc::new(Crypt::ephemeral()));
    let (session, _) = session(&db).await;
    let (status, _) = post(&app, "/admin/posts/new", &session, "title=Launch&slug=").await;
    assert!(status.is_redirection());

    let (status, body) = post(
        &app,
        "/admin/posts/new",
        &session,
        "title=Another&slug=Launch",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body.contains("launch is already used by another record."),
        "{body}"
    );
    assert_eq!(posts(&db).await.len(), 1);
}

#[tokio::test]
async fn an_unchanged_slug_on_an_edit_is_kept() {
    let (db, _guard) = test_db().await;
    let app = app(&db, Arc::new(Crypt::ephemeral()));
    let (session, _) = session(&db).await;
    post(&app, "/admin/posts/new", &session, "title=Notes&slug=").await;
    let (status, body) = post(
        &app,
        "/admin/posts/1/edit",
        &session,
        "title=Notes+renamed&slug=notes&api_key=",
    )
    .await;
    assert!(status.is_redirection(), "{status}: {body}");
    let rows = posts(&db).await;
    assert_eq!(rows[0].text("slug"), Some("notes"));
    assert_eq!(rows[0].text("title"), Some("Notes renamed"));
}
