//! Create the `backend_user_groups` join table.

use laterite_core::strata::*;

use crate::schema::{BackendGroups, BackendUserGroups, BackendUsers};

/// Creates the `backend_user_groups` join table, one row a membership.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0021_create_backend_user_groups"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendUserGroups::Table)
                .if_not_exists()
                // An id of its own, so the admin's membership section can name
                // a row.
                .col(
                    ColumnDef::new(BackendUserGroups::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(BackendUserGroups::BackendUserId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackendUserGroups::BackendGroupId)
                        .big_integer()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendUserGroups::Table, BackendUserGroups::BackendUserId)
                        .to(BackendUsers::Table, BackendUsers::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(BackendUserGroups::Table, BackendUserGroups::BackendGroupId)
                        .to(BackendGroups::Table, BackendGroups::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await?;
        s.exec(
            Index::create()
                .name("backend_user_groups_member_idx")
                .table(BackendUserGroups::Table)
                .col(BackendUserGroups::BackendUserId)
                .col(BackendUserGroups::BackendGroupId)
                .unique()
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendUserGroups::Table).to_owned())
            .await
    }
}
