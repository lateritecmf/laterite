//! Create the `backend_groups` table.

use laterite_core::strata::*;

use crate::schema::BackendGroups;

/// Creates the `backend_groups` table: named sets of operators, for
/// addressing people together (sharing, mail), never for permissions.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0020_create_backend_groups"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendGroups::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendGroups::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                // Unique, so a bounded key rather than `text`, which MySQL
                // cannot index.
                .col(key_col(BackendGroups::Code).not_null().unique_key())
                .col(key_col(BackendGroups::Name).not_null().unique_key())
                .col(ColumnDef::new(BackendGroups::Description).text())
                .col(
                    bool_col(BackendGroups::IsNewUserDefault)
                        .not_null()
                        .default(0),
                )
                .col(ColumnDef::new(BackendGroups::CreatedAt).text().not_null())
                .col(ColumnDef::new(BackendGroups::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendGroups::Table).to_owned())
            .await
    }
}
