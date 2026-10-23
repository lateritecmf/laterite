//! Create the `schedule_runs` table.

use crate::schedule::store::ScheduleRuns;
use crate::strata::*;

/// Creates `schedule_runs`, one row per entry, holding its last run.
pub struct Migration;

#[async_trait(?Send)]
impl crate::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_schedule_runs"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(ScheduleRuns::Table)
                .if_not_exists()
                .col(key_col(ScheduleRuns::Name).not_null().primary_key())
                .col(ColumnDef::new(ScheduleRuns::LastStartedAt).text())
                .col(ColumnDef::new(ScheduleRuns::LastFinishedAt).text())
                .col(key_col(ScheduleRuns::LastStatus).not_null())
                .col(ColumnDef::new(ScheduleRuns::LastError).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(ScheduleRuns::Table).to_owned())
            .await
    }
}
