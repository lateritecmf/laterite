//! The run table's queries.

use chrono::{SecondsFormat, Utc};
use laterite_core::query::{bind_values, build, insert_returning_id};
use laterite_core::{Actor, AnyRowExt, Db};
use sea_query::{Expr, Query};

use crate::run::{Counts, Note, Run, RunSpec, Status};
use crate::schema::ImportRuns;

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// A spec or a note list as the row stores it. These types carry nothing a
/// serializer refuses, so a failure here is a bug worth the panic.
fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("a run's spec and notes serialize")
}

/// Records a run as started, returning its id.
pub(crate) async fn insert(db: &Db, spec: &RunSpec, actor: &Actor) -> Result<i64, sqlx::Error> {
    let stmt = Query::insert()
        .into_table(ImportRuns::Table)
        .columns([
            ImportRuns::Resource,
            ImportRuns::MediaId,
            ImportRuns::ActorUserId,
            ImportRuns::ActorLabel,
            ImportRuns::Status,
            ImportRuns::Spec,
            ImportRuns::Notes,
            ImportRuns::StartedAt,
        ])
        .values_panic([
            spec.resource.clone().into(),
            spec.media_id.into(),
            actor.user_id().into(),
            actor.label().to_string().into(),
            Status::Running.as_str().into(),
            json(spec).into(),
            "[]".into(),
            now().into(),
        ])
        .to_owned();
    insert_returning_id(db, stmt, ImportRuns::Id).await
}

/// Writes the counts so far, while the run is still going.
pub(crate) async fn progress(db: &Db, id: i64, counts: &Counts) -> Result<(), sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(ImportRuns::Table)
            .values(count_values(counts))
            .and_where(Expr::col(ImportRuns::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Closes the run with its final counts and every note.
pub(crate) async fn finish(
    db: &Db,
    id: i64,
    counts: &Counts,
    notes: &[Note],
) -> Result<(), sqlx::Error> {
    // Scoped so the builder's values are dropped before the await: they hold
    // reference-counted identifiers, and a live one across the await would make
    // the run's future non-`Send`.
    let (sql, values) = {
        let mut set = count_values(counts);
        set.push((ImportRuns::Status, Status::Done.as_str().into()));
        set.push((ImportRuns::Notes, json(&notes).into()));
        set.push((ImportRuns::FinishedAt, now().into()));
        build(
            db.backend,
            Query::update()
                .table(ImportRuns::Table)
                .values(set)
                .and_where(Expr::col(ImportRuns::Id).eq(id))
                .to_owned(),
        )
    };
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Closes the run as stopped by `failure`, keeping whatever counts it reached.
pub(crate) async fn fail(db: &Db, id: i64, failure: &str) -> Result<(), sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(ImportRuns::Table)
            .values([
                (ImportRuns::Status, Status::Failed.as_str().into()),
                (ImportRuns::Failure, failure.to_string().into()),
                (ImportRuns::FinishedAt, now().into()),
            ])
            .and_where(Expr::col(ImportRuns::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

fn count_values(counts: &Counts) -> Vec<(ImportRuns, sea_query::SimpleExpr)> {
    vec![
        (ImportRuns::Processed, counts.processed.into()),
        (ImportRuns::Created, counts.created.into()),
        (ImportRuns::Updated, counts.updated.into()),
        (ImportRuns::Skipped, counts.skipped.into()),
        (ImportRuns::Errors, counts.errors.into()),
    ]
}

/// One run by id, or `None` when there is no such run.
pub(crate) async fn find(db: &Db, id: i64) -> Result<Option<Run>, sqlx::Error> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                ImportRuns::Id,
                ImportRuns::Resource,
                ImportRuns::MediaId,
                ImportRuns::ActorLabel,
                ImportRuns::Status,
                ImportRuns::Processed,
                ImportRuns::Created,
                ImportRuns::Updated,
                ImportRuns::Skipped,
                ImportRuns::Errors,
                ImportRuns::Notes,
                ImportRuns::Failure,
                ImportRuns::StartedAt,
                ImportRuns::FinishedAt,
            ])
            .from(ImportRuns::Table)
            .and_where(Expr::col(ImportRuns::Id).eq(id))
            .to_owned(),
    );
    let Some(row) = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?
    else {
        return Ok(None);
    };
    let status = row.get_text("status")?;
    let notes: Vec<Note> = serde_json::from_str(&row.get_text("notes")?).unwrap_or_default();
    Ok(Some(Run {
        id: row.get_int("id")?,
        resource: row.get_text("resource")?,
        media_id: row.get_int_opt("media_id")?,
        actor_label: row.get_text("actor_label")?,
        status: Status::parse(&status).unwrap_or(Status::Failed),
        processed: row.get_int("processed")?,
        created: row.get_int("created")?,
        updated: row.get_int("updated")?,
        skipped: row.get_int("skipped")?,
        errors: row.get_int("errors")?,
        notes,
        failure: row.get_text_opt("failure")?,
        started_at: row.get_text("started_at")?,
        finished_at: row.get_text_opt("finished_at")?,
    }))
}
