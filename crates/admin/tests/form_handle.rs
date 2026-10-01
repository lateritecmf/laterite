//! A screen writing a resource's records through its form from code. Imports
//! only the public API.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axum::Router;
use laterite_admin::form::FormHandle;
use laterite_admin::persist::SaveError;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuditListener, AuthConfig, AuthService};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, Actor, AnyRowExt, CatalogStore, Db, MigrationSet, ModelListenerReg};
use sea_query::{Alias, Expr, Query};

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

/// The posts as (id, code, name), in id order.
async fn rows(db: &Db) -> Vec<(i64, String, String)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Alias::new("id"), Alias::new("code"), Alias::new("name")])
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
                row.get_int("id").unwrap(),
                row.get_text("code").unwrap(),
                row.get_text("name").unwrap(),
            )
        })
        .collect()
}

/// A screen that keeps what the context handed it, so the test can call it
/// after the boot.
struct Keeper(Arc<OnceLock<(Option<FormHandle>, Option<FormHandle>)>>);

impl Screen for Keeper {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let _ = self.0.set((ctx.form("/posts"), ctx.form("/nowhere")));
        Router::new()
    }
}

/// Boots the admin with the posts resource and a screen, and returns what the
/// screen was given for `/posts` and for a path nothing mounts at.
fn boot(db: &Db) -> (Option<FormHandle>, Option<FormHandle>) {
    let kept = Arc::new(OnceLock::new());
    let _ = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/form_handle.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
            listeners: vec![ModelListenerReg::all(Arc::new(AuditListener))],
            screens: vec![ScreenReg::new(
                "/import",
                "acme.import",
                Arc::new(Keeper(kept.clone())),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
    Arc::try_unwrap(kept)
        .ok()
        .and_then(|cell| cell.into_inner())
        .expect("the screen mounted")
}

fn posts(db: &Db) -> FormHandle {
    boot(db).0.expect("the posts resource has a form")
}

fn data(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (String::from(*k), String::from(*v)))
        .collect()
}

/// The audit entries left on `posts`, as (action, actor) pairs.
async fn audit(db: &Db) -> Vec<(String, String)> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([Alias::new("action"), Alias::new("actor_username")])
            .from(Alias::new("backend_audit_log"))
            .and_where(Expr::col(Alias::new("target_type")).eq("posts"))
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
                row.get_text("action").unwrap(),
                row.get_text("actor_username").unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_screen_is_handed_the_forms_the_resources_mounted() {
    let (db, _guard) = test_db().await;
    let (posts, nowhere) = boot(&db);
    let posts = posts.expect("the posts resource has a form");
    assert!(nowhere.is_none(), "a path nothing mounts at has no form");
    assert_eq!(posts.entity(), "posts");
    assert_eq!(posts.id_field(), "id");
    let names: Vec<&str> = posts.fields().iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["code", "name"]);
}

#[tokio::test]
async fn a_create_through_the_form_is_saved_and_audited() {
    let (db, _guard) = test_db().await;
    let posts = posts(&db);
    let actor = Actor::system("acme.import");

    let id = posts
        .create(data(&[("code", "launch"), ("name", "Launch")]), &actor)
        .await
        .expect("saved");

    assert_eq!(
        rows(&db).await,
        [(id, "launch".to_string(), "Launch".to_string())]
    );
    // The same listeners as a submit: the audit trail names the write and the actor.
    assert_eq!(
        audit(&db).await,
        [(
            "backend.posts.create".to_string(),
            "acme.import".to_string()
        )]
    );
}

#[tokio::test]
async fn the_rules_refuse_what_they_would_refuse_on_submit() {
    let (db, _guard) = test_db().await;
    let posts = posts(&db);
    let actor = Actor::system("acme.import");

    let refused = posts.create(data(&[("code", "launch")]), &actor).await;

    match refused {
        Err(SaveError::Invalid(bag)) => {
            assert!(!bag.messages("name").is_empty(), "name is required");
            assert!(bag.messages("code").is_empty());
        }
        other => panic!("expected the rules to refuse, got {other:?}"),
    }
    assert!(rows(&db).await.is_empty(), "a refused write stores nothing");
    assert!(
        audit(&db).await.is_empty(),
        "a refused write leaves no entry"
    );
}

#[tokio::test]
async fn an_update_through_the_form_changes_the_record() {
    let (db, _guard) = test_db().await;
    let posts = posts(&db);
    let actor = Actor::system("acme.import");
    let id = posts
        .create(data(&[("code", "launch"), ("name", "Launch")]), &actor)
        .await
        .unwrap();

    posts
        .update(
            id,
            data(&[("code", "launch"), ("name", "Relaunch")]),
            &actor,
        )
        .await
        .expect("updated");

    assert_eq!(
        rows(&db).await,
        [(id, "launch".to_string(), "Relaunch".to_string())]
    );
    let actions: Vec<String> = audit(&db).await.into_iter().map(|(a, _)| a).collect();
    assert_eq!(actions, ["backend.posts.create", "backend.posts.update"]);
}
