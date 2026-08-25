//! Create the `backend_sessions` table and its user index.

use laterite_core::strata::*;

use crate::schema::{BackendSessions, BackendUsers};

/// Creates the `backend_sessions` table and its user index.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0004_create_backend_sessions"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendSessions::Table)
                .if_not_exists()
                .col(key_col(BackendSessions::TokenHash).not_null().primary_key())
                .col(
                    ColumnDef::new(BackendSessions::BackendUserId)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(BackendSessions::CreatedAt).text().not_null())
                .col(
                    ColumnDef::new(BackendSessions::LastSeenAt)
                        .text()
                        .not_null(),
                )
                .col(ColumnDef::new(BackendSessions::ExpiresAt).text().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendSessions::Table, BackendSessions::BackendUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("backend_sessions_user_idx")
                .table(BackendSessions::Table)
                .col(BackendSessions::BackendUserId)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendSessions::Table).to_owned())
            .await
    }
}
