//! Add `channel` to `backend_audit_log`.

use laterite_core::strata::*;

use crate::schema::BackendAuditLog;

/// What acted for the operator (an assistant, an outside agent), when they did
/// not act themselves. Null on every entry written by hand.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0022_add_backend_audit_channel"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendAuditLog::Table)
                .add_column(ColumnDef::new(BackendAuditLog::Channel).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendAuditLog::Table)
                .drop_column(BackendAuditLog::Channel)
                .to_owned(),
        )
        .await
    }
}
