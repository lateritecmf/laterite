//! Create the append-only `backend_audit_log` table and its lookup index.

use laterite_core::strata::*;

use crate::schema::{BackendAuditLog, BackendUsers};

/// Creates the `backend_audit_log` table and its lookup index.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0010_create_backend_audit_log"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendAuditLog::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendAuditLog::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                // The actor id is nullable (a system action has none) and set null
                // if the user is later removed; the username is snapshotted so the
                // entry stays legible after the account is gone.
                .col(ColumnDef::new(BackendAuditLog::ActorUserId).big_integer())
                .col(
                    ColumnDef::new(BackendAuditLog::ActorUsername)
                        .text()
                        .not_null(),
                )
                .col(ColumnDef::new(BackendAuditLog::Action).text().not_null())
                .col(ColumnDef::new(BackendAuditLog::TargetType).text())
                .col(ColumnDef::new(BackendAuditLog::TargetId).text())
                .col(ColumnDef::new(BackendAuditLog::Detail).text())
                // Indexed below, so a bounded key rather than `text`, which MySQL
                // cannot index.
                .col(key_col(BackendAuditLog::CreatedAt).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendAuditLog::Table, BackendAuditLog::ActorUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::SetNull),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("backend_audit_log_created_idx")
                .table(BackendAuditLog::Table)
                .col(BackendAuditLog::CreatedAt)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendAuditLog::Table).to_owned())
            .await
    }
}
