//! Add `password_changed_at` to `backend_users`.

use laterite_core::strata::*;

use crate::schema::BackendUsers;

/// When the password was last set, shown to the operator and to administrators.
/// Empty for an account whose password has not changed since this column arrived.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0017_add_backend_user_password_changed_at"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(ColumnDef::new(BackendUsers::PasswordChangedAt).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::PasswordChangedAt)
                .to_owned(),
        )
        .await
    }
}
