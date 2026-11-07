//! Seeders: run in order, filtered by module, and idempotent through
//! `insert_missing`.
use laterite_core::seed::{self, insert_missing, Seeder, SeederSet};
use laterite_core::strata::*;
use laterite_core::testing::connect_test;
use laterite_core::{MigrationSet, Record};
use sea_query::Alias;

struct CreateStatuses;

#[async_trait(?Send)]
impl Migration for CreateStatuses {
    fn name(&self) -> &str {
        "0001_create_statuses"
    }

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("statuses"))
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(Alias::new("code")).not_null())
                .col(ColumnDef::new(Alias::new("label")).text().not_null())
                .to_owned(),
        )
        .await
    }
}

struct Statuses;

#[laterite_core::strata::async_trait]
impl Seeder for Statuses {
    fn name(&self) -> &str {
        "statuses"
    }

    async fn run(&self, db: &Db) -> CoreResult<()> {
        for (code, label) in [("draft", "Draft"), ("live", "Live")] {
            let mut rec = Record::new("statuses");
            rec.set("code", code).set("label", label);
            insert_missing(db, &rec, &["code"]).await?;
        }
        Ok(())
    }
}

async fn count(db: &Db) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM statuses")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    n
}

#[tokio::test]
async fn a_seeder_run_twice_leaves_one_row_per_key() {
    let (db, _guard) = connect_test(&[MigrationSet::new(
        "acme.blog",
        vec![Box::new(CreateStatuses)],
    )])
    .await;
    let sets = [SeederSet::new("acme.blog", vec![Box::new(Statuses)])];
    let ran = seed::run(&db, &sets, None).await.unwrap();
    assert_eq!(ran, ["acme.blog/statuses"]);
    seed::run(&db, &sets, None).await.unwrap();
    assert_eq!(count(&db).await, 2);
}

#[tokio::test]
async fn a_module_filter_runs_only_that_module() {
    let (db, _guard) = connect_test(&[MigrationSet::new(
        "acme.blog",
        vec![Box::new(CreateStatuses)],
    )])
    .await;
    let sets = [SeederSet::new("acme.blog", vec![Box::new(Statuses)])];
    assert!(seed::run(&db, &sets, Some("acme.shop"))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(count(&db).await, 0);
    assert_eq!(
        seed::run(&db, &sets, Some("acme.blog"))
            .await
            .unwrap()
            .len(),
        1
    );
}
