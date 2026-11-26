use laterite_core::strata::*;

use crate::eventlog::BackendEventLog;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_backend_event_log"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendEventLog::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendEventLog::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(BackendEventLog::Level).not_null())
                .col(key_col(BackendEventLog::Target).not_null())
                .col(ColumnDef::new(BackendEventLog::Message).text().not_null())
                .col(ColumnDef::new(BackendEventLog::Detail).text())
                .col(
                    ColumnDef::new(BackendEventLog::Count)
                        .big_integer()
                        .not_null()
                        .default(1),
                )
                .col(ColumnDef::new(BackendEventLog::FirstSeen).text().not_null())
                .col(ColumnDef::new(BackendEventLog::LastSeen).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("idx_backend_event_log_last_seen")
                .table(BackendEventLog::Table)
                .col(BackendEventLog::LastSeen)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendEventLog::Table).to_owned())
            .await
    }
}
