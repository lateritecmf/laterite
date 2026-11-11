//! Make a job's unique key unique in the table, so two pushes at the same
//! moment cannot both get in.

use crate::queue::database::Jobs;
use crate::strata::*;

/// Adds a unique index on `jobs.unique_key`. Rows with no key are unaffected:
/// every supported database lets a unique index hold any number of nulls.
pub struct Migration;

#[async_trait(?Send)]
impl crate::Migration for Migration {
    fn name(&self) -> &str {
        "0002_add_jobs_unique_key_index"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Index::create()
                .name("jobs_unique_key")
                .table(Jobs::Table)
                .col(Jobs::UniqueKey)
                .unique()
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Index::drop()
                .name("jobs_unique_key")
                .table(Jobs::Table)
                .to_owned(),
        )
        .await
    }
}
