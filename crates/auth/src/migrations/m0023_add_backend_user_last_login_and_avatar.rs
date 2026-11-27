//! Add `last_login_at` and `avatar_id` to `backend_users`.

use laterite_core::strata::*;

use crate::schema::BackendUsers;

/// When the account last signed in, and the media record of its picture.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0023_add_backend_user_last_login_and_avatar"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(ColumnDef::new(BackendUsers::LastLoginAt).text())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(ColumnDef::new(BackendUsers::AvatarId).big_integer())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::AvatarId)
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::LastLoginAt)
                .to_owned(),
        )
        .await
    }
}
