//! Add `color` to `backend_roles`.

use laterite_core::strata::*;

use crate::schema::BackendRoles;

/// The colour a role is marked with, as `#rrggbb`.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0024_add_backend_role_color"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRoles::Table)
                .add_column(ColumnDef::new(BackendRoles::Color).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRoles::Table)
                .drop_column(BackendRoles::Color)
                .to_owned(),
        )
        .await
    }
}
