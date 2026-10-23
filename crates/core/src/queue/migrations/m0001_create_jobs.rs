//! Create the `jobs` and `failed_jobs` tables.

use crate::queue::database::{FailedJobs, Jobs};
use crate::strata::*;

/// Creates `jobs`, one row per waiting or running job, indexed by queue and
/// claim, and `failed_jobs`, one row per job that gave up.
pub struct Migration;

#[async_trait(?Send)]
impl crate::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_jobs"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Jobs::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Jobs::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(Jobs::Queue).not_null())
                .col(key_col(Jobs::Name).not_null())
                .col(ColumnDef::new(Jobs::Payload).text().not_null())
                .col(ColumnDef::new(Jobs::Attempts).integer().not_null())
                .col(ColumnDef::new(Jobs::AvailableAt).text().not_null())
                .col(ColumnDef::new(Jobs::ReservedAt).text())
                .col(key_col(Jobs::ReservedBy))
                .col(key_col(Jobs::UniqueKey))
                .col(ColumnDef::new(Jobs::CreatedAt).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("jobs_queue_reserved_at")
                .table(Jobs::Table)
                .col(Jobs::Queue)
                .col(Jobs::ReservedAt)
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::create()
                .table(FailedJobs::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(FailedJobs::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(FailedJobs::Queue).not_null())
                .col(key_col(FailedJobs::Name).not_null())
                .col(ColumnDef::new(FailedJobs::Payload).text().not_null())
                .col(ColumnDef::new(FailedJobs::Error).text().not_null())
                .col(ColumnDef::new(FailedJobs::FailedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(FailedJobs::Table).to_owned())
            .await?;
        s.exec(Table::drop().table(Jobs::Table).to_owned()).await
    }
}
