//! Every framework write path moves its cache namespace on: a record saved
//! or deleted through the admin bumps `records::<entity>`, and a setting
//! saved through the store bumps the settings namespace. Imports only the
//! framework's public API.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::form::{FormConfig, FormField};
use laterite_admin::list::{ListColumn, ListConfig};
use laterite_admin::settings::{BrandSetting, Settings, NAMESPACE as SETTINGS};
use laterite_admin::{router, AdminConfig, Contributions, Permission, Resource};
use laterite_auth::{AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::cache::records;
use laterite_core::strata::*;
use laterite_core::{t, Cache, CatalogStore, Db};
use sea_query::Alias;
use std::sync::Arc;
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

struct Notes;

#[async_trait(?Send)]
impl laterite_core::Migration for Notes {
    fn name(&self) -> &str {
        "0001_create_notes"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("notes"))
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

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(MigrationSet::new("acme", vec![Box::new(Notes)]));
    laterite_core::testing::connect_test(&migrations).await
}

fn app(db: &Db, cache: &Cache) -> Router {
    let mut config = AdminConfig::default();
    config.cache = Some(cache.clone());
    let resource = Resource::new(
        "/notes",
        "Notes",
        ListConfig::new("notes", "Notes", vec![ListColumn::new("title", "Title")]).deletable(),
    )
    .form(FormConfig::new(
        "notes",
        "Note",
        "/notes",
        "id",
        vec![FormField::text("title", "Title")],
    ))
    .permission("acme.manage_notes");
    router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource],
            permissions: vec![Permission::new(
                "acme.manage_notes",
                t!("Manage notes"),
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

async fn first_id(db: &Db) -> i64 {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Alias::new("id"))
            .from(Alias::new("notes"))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::Row::try_get::<i64, _>(&row, "id").unwrap()
}

#[tokio::test]
async fn a_save_and_a_delete_through_the_admin_bump_the_entity() {
    let (db, _guard) = test_db().await;
    let cache = Cache::local();
    let app = app(&db, &cache);
    let root = session(&db).await;
    let notes = records("notes");

    let start = cache.generation(&notes).await.unwrap();
    assert_eq!(
        post(&app, "/admin/notes/new", &root, "title=First").await,
        StatusCode::SEE_OTHER
    );
    let after_create = cache.current(&notes).unwrap();
    assert_ne!(after_create, start, "a create moves the entity on");

    let id = first_id(&db).await;
    assert_eq!(
        post(
            &app,
            &format!("/admin/notes/{id}/edit"),
            &root,
            "title=Renamed"
        )
        .await,
        StatusCode::SEE_OTHER
    );
    let after_update = cache.current(&notes).unwrap();
    assert_ne!(after_update, after_create, "an update moves the entity on");

    assert_eq!(
        post(&app, "/admin/notes/delete", &root, &format!("id={id}")).await,
        StatusCode::SEE_OTHER
    );
    assert_ne!(
        cache.current(&notes).unwrap(),
        after_update,
        "a delete moves the entity on"
    );
}

#[tokio::test]
async fn a_setting_saved_through_the_store_bumps_the_settings_namespace() {
    let (db, _guard) = test_db().await;
    let cache = Cache::local();
    let settings = Settings::new(db.clone(), cache.clone());
    let start = cache.generation(SETTINGS).await.unwrap();
    settings
        .save(&BrandSetting {
            app_name: "Acme".to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_ne!(cache.current(SETTINGS).unwrap(), start);
    // A cached value built from the namespace misses from now on.
    cache
        .put(
            "brand",
            b"old".to_vec(),
            &[SETTINGS],
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap();
    settings
        .set("acme.flags", &serde_json::json!({ "beta": true }))
        .await
        .unwrap();
    assert!(cache.get("brand").await.is_none());
}
