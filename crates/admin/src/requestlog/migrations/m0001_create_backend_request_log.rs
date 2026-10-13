use laterite_core::strata::*;

use crate::requestlog::BackendRequestLog;

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_backend_request_log"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendRequestLog::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendRequestLog::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(BackendRequestLog::Method).not_null())
                .col(ColumnDef::new(BackendRequestLog::Path).text().not_null())
                .col(ColumnDef::new(BackendRequestLog::Status).integer().not_null())
                .col(ColumnDef::new(BackendRequestLog::Count).big_integer().not_null().default(1))
                .col(ColumnDef::new(BackendRequestLog::FirstSeen).text().not_null())
                .col(ColumnDef::new(BackendRequestLog::LastSeen).text().not_null())
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("idx_backend_request_log_last_seen")
                .table(BackendRequestLog::Table)
                .col(BackendRequestLog::LastSeen)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendRequestLog::Table).to_owned()).await
    }
}
