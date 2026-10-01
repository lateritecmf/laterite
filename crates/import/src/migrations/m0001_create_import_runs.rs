//! Create the import run table.

use laterite_core::strata::*;

use crate::schema::ImportRuns;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_import_runs"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(ImportRuns::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(ImportRuns::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(ImportRuns::Resource).not_null())
                .col(ColumnDef::new(ImportRuns::MediaId).big_integer())
                .col(ColumnDef::new(ImportRuns::ActorUserId).big_integer())
                .col(key_col(ImportRuns::ActorLabel).not_null())
                .col(key_col(ImportRuns::Status).not_null())
                .col(ColumnDef::new(ImportRuns::Spec).text().not_null())
                .col(ColumnDef::new(ImportRuns::Processed).big_integer().not_null().default(0))
                .col(ColumnDef::new(ImportRuns::Created).big_integer().not_null().default(0))
                .col(ColumnDef::new(ImportRuns::Updated).big_integer().not_null().default(0))
                .col(ColumnDef::new(ImportRuns::Skipped).big_integer().not_null().default(0))
                .col(ColumnDef::new(ImportRuns::Errors).big_integer().not_null().default(0))
                .col(ColumnDef::new(ImportRuns::Notes).text().not_null())
                .col(ColumnDef::new(ImportRuns::Failure).text())
                .col(ColumnDef::new(ImportRuns::StartedAt).text().not_null())
                .col(ColumnDef::new(ImportRuns::FinishedAt).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(ImportRuns::Table).to_owned()).await
    }
}
