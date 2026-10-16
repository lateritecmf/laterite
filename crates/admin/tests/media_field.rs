//! The `media` field: ids stored one way or the other, the record linked to
//! its files after a save, and the picker the field opens. Imports only the
//! public API.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::FormHandle;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::strata::*;
use laterite_core::{t, Actor, CatalogStore, Db, MigrationSet};
use laterite_media::{link, record, NewMedia, NewMediaExtra, Owner};
use sea_query::Alias;
use tower::ServiceExt;

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
                .col(ColumnDef::new(Alias::new("cover_id")).big_integer().null())
                .col(ColumnDef::new(Alias::new("gallery")).text().null())
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

struct Keeper(Arc<OnceLock<Option<FormHandle>>>);

impl Screen for Keeper {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let _ = self.0.set(ctx.form("/posts"));
        Router::new()
    }
}

fn boot(db: &Db) -> (Router, FormHandle) {
    let kept = Arc::new(OnceLock::new());
    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/media_field.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            screens: vec![ScreenReg::new(
                "/keeper",
                "acme.keeper",
                Arc::new(Keeper(kept.clone())),
            )],
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    );
    let form = Arc::try_unwrap(kept)
        .ok()
        .and_then(|cell| cell.into_inner())
        .flatten()
        .expect("the posts form");
    (app, form)
}

async fn file(db: &Db, name: &str) -> i64 {
    record::create_with(
        db,
        NewMedia {
            disk: "public".into(),
            hash: format!("{:0>64}", name.len()),
            size: 10,
            content_type: "image/png".into(),
            original_name: name.into(),
            in_library: true,
            created_by: None,
        },
        NewMediaExtra::default(),
    )
    .await
    .unwrap()
}

fn data(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (String::from(*k), String::from(*v)))
        .collect()
}

async fn linked(db: &Db, id: i64, field: &str) -> Vec<i64> {
    link::of_owner(
        db,
        Owner {
            owner_type: "posts",
            owner_id: &id.to_string(),
            field,
        },
    )
    .await
    .unwrap()
    .into_iter()
    .map(|l| l.media_id)
    .collect()
}

#[tokio::test]
async fn a_saved_media_field_links_the_record_to_its_files() {
    let (db, _guard) = test_db().await;
    let (_, posts) = boot(&db);
    let actor = Actor::system("acme.test");
    let a = file(&db, "a.png").await;
    let b = file(&db, "bb.png").await;
    let c = file(&db, "ccc.png").await;

    let id = posts
        .create(
            data(&[
                ("code", "launch"),
                ("cover_id", &a.to_string()),
                ("gallery", &format!("{b},{c}")),
            ]),
            &actor,
        )
        .await
        .expect("saved");
    assert_eq!(linked(&db, id, "cover_id").await, [a]);
    assert_eq!(linked(&db, id, "gallery").await, [b, c]);
    assert_eq!(link::users_of(&db, a).await.unwrap()[0].owner_type, "posts");

    posts
        .update(
            id,
            data(&[
                ("code", "launch"),
                ("cover_id", ""),
                ("gallery", &c.to_string()),
            ]),
            &actor,
        )
        .await
        .expect("updated");
    assert!(linked(&db, id, "cover_id").await.is_empty(), "cleared");
    assert_eq!(linked(&db, id, "gallery").await, [c]);
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
    svc.authenticate("root", "rootpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token
}

async fn get(app: &Router, path: &str, token: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", format!("laterite_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// The field renders as its widget with the picker's endpoints, the picker
/// fragment offers the library's files, and the lookup answers the tiles.
#[tokio::test]
async fn the_field_renders_and_its_picker_offers_the_library() {
    let (db, _guard) = test_db().await;
    let (app, _) = boot(&db);
    let token = session(&db).await;
    let a = file(&db, "a.png").await;

    let (status, html) = get(&app, "/admin/posts/new", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"data-lat-widget="media-field""#)
            && html.contains(r#"data-pick="/admin/media/pick""#)
            && html.contains(r#"data-mode="image""#)
            && html.contains(r#"data-max="4""#),
        "{html}"
    );

    let (status, fragment) = get(&app, "/admin/media/pick?mode=image", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        fragment.contains("data-pick-file")
            && fragment.contains("a.png")
            && !fragment.contains("<html"),
        "{fragment}"
    );

    let (status, json) = get(&app, &format!("/admin/media/resolve?ids={a},999"), &token).await;
    assert_eq!(status, StatusCode::OK);
    let items: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["name"], "a.png");
    assert!(items[0]["thumb"].as_str().unwrap().contains("/thumb/"));
}
