//! Create the `backend_remember_tokens` table and its user index.

use laterite_core::strata::*;

use crate::schema::{BackendRememberTokens, BackendUsers};

/// Creates the `backend_remember_tokens` table and its user index.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0012_create_backend_remember_tokens"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendRememberTokens::Table)
                .if_not_exists()
                // The cookie carries `selector:verifier`. The selector is the
                // lookup key and is stored plain; only the verifier is secret,
                // and only its hash is kept, so a leaked table grants nothing.
                .col(
                    key_col(BackendRememberTokens::Selector)
                        .not_null()
                        .primary_key(),
                )
                .col(
                    key_col(BackendRememberTokens::VerifierHash)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackendRememberTokens::BackendUserId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackendRememberTokens::CreatedAt)
                        .text()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackendRememberTokens::ExpiresAt)
                        .text()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(
                            BackendRememberTokens::Table,
                            BackendRememberTokens::BackendUserId,
                        )
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("backend_remember_tokens_user_idx")
                .table(BackendRememberTokens::Table)
                .col(BackendRememberTokens::BackendUserId)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::drop()
                .table(BackendRememberTokens::Table)
                .to_owned(),
        )
        .await
    }
}
