//! A run from a CSV through a resource's form, polled to its report. Imports
//! only the public API.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use laterite_admin::axum::Router;
use laterite_admin::form::FormHandle;
use laterite_admin::import::{
    find, start_inline, Duplicates, Match, NoteKind, RunError, RunSpec, Status,
};
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{AuthConfig, AuthService};
use laterite_core::query::{bind_values, build};
use laterite_core::strata::*;
use laterite_core::{t, Actor, AnyRowExt, CatalogStore, Db, MigrationSet};
use laterite_media::{stream_of, ByteStream};
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
                .col(ColumnDef::new(Alias::new("note")).text())
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

/// A screen that keeps the form it was handed, so the test can run through it.
struct Keeper(Arc<OnceLock<FormHandle>>);

impl Screen for Keeper {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let _ = self.0.set(ctx.form("/posts").expect("the posts form"));
        Router::new()
    }
}

fn posts(db: &Db) -> FormHandle {
    let kept = Arc::new(OnceLock::new());
    let _ = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            resources: vec![resource!("tests/descriptors/import_engine.yaml")],
            permissions: vec![Permission::new(
                "acme.manage_posts",
                t!("Manage posts"),
                t!("Acme"),
            )],
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

fn csv(text: &str) -> ByteStream {
    stream_of(text.as_bytes().to_vec())
}

fn actor() -> Actor {
    Actor::system("acme.import")
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

/// Polls the run until it leaves `Running`.
async fn finished(db: &Db, id: i64) -> laterite_admin::import::Run {
    for _ in 0..500 {
        let run = find(db, id).await.unwrap().expect("the run row");
        if run.status != Status::Running {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the run did not finish");
}

fn code_and_name() -> Vec<Match> {
    vec![Match::new(0, "code"), Match::new(1, "name")]
}

#[tokio::test]
async fn rows_are_written_through_the_form_and_reported() {
    let (db, _guard) = test_db().await;
    let form = posts(&db);
    let file = "code,name\nlaunch,Launch\n,No code\n\n relaunch ,Relaunch\n";

    let id = start_inline(
        &db,
        form,
        actor(),
        RunSpec::new("/posts", code_and_name()),
        csv(file),
    )
    .await
    .expect("started");
    let run = finished(&db, id).await;

    assert_eq!(run.status, Status::Done);
    assert_eq!((run.processed, run.created, run.errors), (3, 2, 1));
    assert_eq!(run.resource, "/posts");
    assert_eq!(run.actor_label, "acme.import");
    assert!(run.finished_at.is_some());
    // The refused row is a note naming its line in the file and its field.
    assert_eq!(run.notes.len(), 1);
    assert_eq!(run.notes[0].row, 3);
    assert_eq!(run.notes[0].kind, NoteKind::Error);
    assert_eq!(run.notes[0].field.as_deref(), Some("code"));
    // Values arrive as the file holds them.
    assert_eq!(
        rows(&db).await,
        [
            ("launch".to_string(), "Launch".to_string(), None),
            (" relaunch ".to_string(), "Relaunch".to_string(), None),
        ]
    );
}

#[tokio::test]
async fn a_duplicate_is_skipped_or_updated_as_the_spec_says() {
    let (db, _guard) = test_db().await;
    let form = posts(&db);
    let mut first = HashMap::new();
    first.insert("code".to_string(), "launch".to_string());
    first.insert("name".to_string(), "Launch".to_string());
    first.insert("note".to_string(), "kept".to_string());
    form.create(first, &actor()).await.unwrap();

    let skip = RunSpec::new("/posts", code_and_name()).duplicates(Duplicates::skip("code"));
    let id = start_inline(
        &db,
        form.clone(),
        actor(),
        skip,
        csv("code,name\nlaunch,Again\nnew,New\n"),
    )
    .await
    .unwrap();
    let run = finished(&db, id).await;
    assert_eq!((run.created, run.skipped, run.errors), (1, 1, 0));
    assert_eq!(run.notes[0].kind, NoteKind::Skipped);
    assert_eq!(run.notes[0].row, 2);

    let update = RunSpec::new("/posts", code_and_name()).duplicates(Duplicates::update("code"));
    let id = start_inline(
        &db,
        form,
        actor(),
        update,
        csv("code,name\nlaunch,Relaunch\n"),
    )
    .await
    .unwrap();
    let run = finished(&db, id).await;
    assert_eq!((run.created, run.updated, run.errors), (0, 1, 0));
    // The file's columns over the record: the note the file never carried stays.
    assert_eq!(
        rows(&db).await,
        [
            (
                "launch".to_string(),
                "Relaunch".to_string(),
                Some("kept".to_string())
            ),
            ("new".to_string(), "New".to_string(), None),
        ]
    );
}

#[tokio::test]
async fn the_file_options_are_honoured() {
    let (db, _guard) = test_db().await;
    let form = posts(&db);
    let spec = RunSpec::new("/posts", code_and_name())
        .first_row_titles(false)
        .delimiter(b';');

    let id = start_inline(
        &db,
        form,
        actor(),
        spec,
        csv("launch;Launch\nrelaunch;Relaunch\n"),
    )
    .await
    .unwrap();
    let run = finished(&db, id).await;

    assert_eq!((run.processed, run.created), (2, 2));
}

#[tokio::test]
async fn a_spec_the_form_cannot_take_never_starts() {
    let (db, _guard) = test_db().await;
    let form = posts(&db);

    let unknown = RunSpec::new("/posts", vec![Match::new(0, "title")]);
    assert!(matches!(
        start_inline(&db, form.clone(), actor(), unknown, csv("")).await,
        Err(RunError::Spec(_))
    ));
    let unfed = RunSpec::new("/posts", code_and_name()).duplicates(Duplicates::skip("note"));
    assert!(matches!(
        start_inline(&db, form.clone(), actor(), unfed, csv("")).await,
        Err(RunError::Spec(_))
    ));
    let empty = RunSpec::new("/posts", vec![]);
    assert!(matches!(
        start_inline(&db, form, actor(), empty, csv("")).await,
        Err(RunError::Spec(_))
    ));
    assert!(
        find(&db, 1).await.unwrap().is_none(),
        "no run row is left behind"
    );
}

#[tokio::test]
async fn a_file_that_cannot_be_read_fails_the_run() {
    let (db, _guard) = test_db().await;
    let form = posts(&db);
    let broken: ByteStream = Box::pin(futures::stream::iter([Err(std::io::Error::other(
        "disk gone",
    ))]));

    let id = start_inline(
        &db,
        form,
        actor(),
        RunSpec::new("/posts", code_and_name()),
        broken,
    )
    .await
    .unwrap();
    let run = finished(&db, id).await;

    assert_eq!(run.status, Status::Failed);
    assert!(run.failure.as_deref().unwrap_or("").contains("disk gone"));
    assert!(rows(&db).await.is_empty());
}

#[tokio::test]
async fn a_dry_run_reports_everything_and_writes_nothing() {
    let (db, _guard) = test_db().await;
    let form = posts(&db);
    let spec = RunSpec::new("/posts", code_and_name()).dry_run(true);

    let id = start_inline(
        &db,
        form,
        actor(),
        spec,
        csv("code,name\nlaunch,Launch\n,No code\n"),
    )
    .await
    .unwrap();
    let run = finished(&db, id).await;

    assert_eq!(run.status, Status::Done);
    assert_eq!((run.created, run.errors), (1, 1), "what an import would do");
    assert!(run.spec.dry_run);
    assert!(rows(&db).await.is_empty());
}

#[tokio::test]
async fn a_blank_cell_keeps_the_value_unless_told_to_clear() {
    let (db, _guard) = test_db().await;
    let form = posts(&db);
    let mut first = HashMap::new();
    first.insert("code".to_string(), "launch".to_string());
    first.insert("name".to_string(), "Launch".to_string());
    first.insert("note".to_string(), "kept".to_string());
    form.create(first, &actor()).await.unwrap();
    let matches = || {
        vec![
            Match::new(0, "code"),
            Match::new(1, "name"),
            Match::new(2, "note"),
        ]
    };
    let file = "code,name,note\nlaunch,Relaunch,\n";

    let keep = RunSpec::new("/posts", matches()).duplicates(Duplicates::update("code"));
    let id = start_inline(&db, form.clone(), actor(), keep, csv(file))
        .await
        .unwrap();
    finished(&db, id).await;
    assert_eq!(
        rows(&db).await,
        [(
            "launch".to_string(),
            "Relaunch".to_string(),
            Some("kept".to_string())
        )]
    );

    let clear = RunSpec::new("/posts", matches())
        .duplicates(Duplicates::update("code"))
        .keep_blank(false);
    let id = start_inline(&db, form, actor(), clear, csv(file))
        .await
        .unwrap();
    finished(&db, id).await;
    assert_eq!(
        rows(&db).await,
        [("launch".to_string(), "Relaunch".to_string(), None)]
    );
}
