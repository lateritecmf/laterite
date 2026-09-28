//! Add `must_change_password` to `backend_users`.

use laterite_core::strata::*;

use crate::schema::BackendUsers;

/// Set on an account holding a temporary password, one made for it by an
/// administrator or the command line; cleared when the operator sets their own.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0019_add_backend_user_must_change_password"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(
                    bool_col(BackendUsers::MustChangePassword)
                        .not_null()
                        .default(0),
                )
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::MustChangePassword)
                .to_owned(),
        )
        .await
    }
}
