//! Add `target_label` to `backend_audit_log`.

use laterite_core::strata::*;

use crate::schema::BackendAuditLog;

/// The target's name as operators know it (a username, a role name),
/// snapshotted on the entry the way the actor's username is.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0018_add_backend_audit_target_label"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendAuditLog::Table)
                .add_column(ColumnDef::new(BackendAuditLog::TargetLabel).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendAuditLog::Table)
                .drop_column(BackendAuditLog::TargetLabel)
                .to_owned(),
        )
        .await
    }
}
