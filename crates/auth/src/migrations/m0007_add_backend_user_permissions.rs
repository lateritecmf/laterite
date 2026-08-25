//! Add the `permissions` column to `backend_users`.

use laterite_core::strata::*;

use crate::schema::BackendUsers;

/// Adds the JSON `permissions` column to `backend_users`.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0007_add_backend_user_permissions"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(
                    ColumnDef::new(BackendUsers::Permissions)
                        .text()
                        .not_null()
                        .default("{}"),
                )
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::Permissions)
                .to_owned(),
        )
        .await
    }
}
