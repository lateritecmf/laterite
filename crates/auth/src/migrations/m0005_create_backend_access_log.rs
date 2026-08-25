//! Create the `backend_access_log` table and its lookup index.

use laterite_core::strata::*;

use crate::schema::{BackendAccessLog, BackendUsers};

/// Creates the `backend_access_log` table and its lookup index.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0005_create_backend_access_log"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendAccessLog::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendAccessLog::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(BackendAccessLog::BackendUserId).big_integer())
                // Indexed below (username + created_at), so both are bounded keys
                // rather than `text`, which MySQL cannot index.
                .col(key_col(BackendAccessLog::UsernameAttempted).not_null())
                .col(ColumnDef::new(BackendAccessLog::Event).text().not_null())
                .col(ColumnDef::new(BackendAccessLog::IpAddress).text())
                .col(ColumnDef::new(BackendAccessLog::UserAgent).text())
                .col(key_col(BackendAccessLog::CreatedAt).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendAccessLog::Table, BackendAccessLog::BackendUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::SetNull),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("backend_access_log_username_idx")
                .table(BackendAccessLog::Table)
                .col(BackendAccessLog::UsernameAttempted)
                .col(BackendAccessLog::CreatedAt)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendAccessLog::Table).to_owned())
            .await
    }
}
