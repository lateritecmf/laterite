//! Add the `is_system` and `description` columns to `backend_roles`.

use laterite_core::strata::*;

use crate::schema::BackendRoles;

/// Marks the roles the framework owns, and gives every role a sentence.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0014_add_backend_role_system"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRoles::Table)
                .add_column(bool_col(BackendRoles::IsSystem).not_null().default(0))
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::alter()
                .table(BackendRoles::Table)
                .add_column(ColumnDef::new(BackendRoles::Description).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRoles::Table)
                .drop_column(BackendRoles::IsSystem)
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::alter()
                .table(BackendRoles::Table)
                .drop_column(BackendRoles::Description)
                .to_owned(),
        )
        .await
    }
}
