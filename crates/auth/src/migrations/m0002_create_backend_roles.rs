//! Create the `backend_roles` table.

use laterite_core::strata::*;

use crate::schema::BackendRoles;

/// Creates the `backend_roles` table.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0002_create_backend_roles"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendRoles::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendRoles::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(BackendRoles::Code).not_null().unique_key())
                .col(ColumnDef::new(BackendRoles::Name).text().not_null())
                .col(
                    ColumnDef::new(BackendRoles::Permissions)
                        .text()
                        .not_null()
                        .default("[]"),
                )
                .col(ColumnDef::new(BackendRoles::CreatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendRoles::Table).to_owned())
            .await
    }
}
