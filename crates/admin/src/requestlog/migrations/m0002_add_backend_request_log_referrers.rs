//! Add `referrers` to `backend_request_log`.

use laterite_core::strata::*;

use crate::requestlog::BackendRequestLog;

/// The pages that linked to the URL, one a line.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0002_add_backend_request_log_referrers"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRequestLog::Table)
                .add_column(ColumnDef::new(BackendRequestLog::Referrers).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRequestLog::Table)
                .drop_column(BackendRequestLog::Referrers)
                .to_owned(),
        )
        .await
    }
}
