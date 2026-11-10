//! A field's `format`: every shape typed is stored as one address, and any
//! other value is refused on the field. Imports only the public API.

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
use sea_query::{Alias, Query};

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
            resources: vec![resource!("tests/descriptors/format_posts.yaml")],
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

#[tokio::test]
async fn a_formatted_field_stores_one_address_and_refuses_the_rest() {
    let (db, _guard) = test_db().await;
    let posts = posts(&db);
    let actor = Actor::system("acme.import");
    for (code, typed) in [
        ("a", "https://www.social.acme/Acme.Band/?ref=1"),
        ("b", "@acme.band"),
        ("c", "social.acme/acme.band"),
    ] {
        posts
            .create(data(&[("code", code), ("name", typed)]), &actor)
            .await
            .unwrap_or_else(|e| panic!("{typed}: {e:?}"));
    }
    let stored: Vec<String> = rows(&db)
        .await
        .into_iter()
        .map(|(_, _, name)| name)
        .collect();
    assert_eq!(stored, vec!["https://www.social.acme/acme.band"; 3]);

    match posts
        .create(
            data(&[("code", "d"), ("name", "https://other.acme/acme")]),
            &actor,
        )
        .await
    {
        Err(SaveError::Invalid(bag)) => {
            let fields: Vec<&str> = bag.iter().map(|(field, _)| field).collect();
            assert_eq!(fields, ["name"]);
        }
        other => panic!("expected the field refused, got {other:?}"),
    }
    assert_eq!(rows(&db).await.len(), 3, "the refused value was not saved");
}
