//! Create the `backend_user_roles` join table.

use laterite_core::strata::*;

use crate::schema::{BackendRoles, BackendUserRoles, BackendUsers};

/// Creates the `backend_user_roles` join table.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0003_create_backend_user_roles"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendUserRoles::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendUserRoles::BackendUserId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackendUserRoles::BackendRoleId)
                        .big_integer()
                        .not_null(),
                )
                .primary_key(
                    Index::create()
                        .col(BackendUserRoles::BackendUserId)
                        .col(BackendUserRoles::BackendRoleId),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendUserRoles::Table, BackendUserRoles::BackendUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendUserRoles::Table, BackendUserRoles::BackendRoleId)
                        .to(BackendRoles::Table, BackendRoles::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendUserRoles::Table).to_owned())
            .await
    }
}
