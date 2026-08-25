//! Create the `backend_users` table.

use laterite_core::strata::*;

use crate::schema::BackendUsers;

/// Creates the `backend_users` table.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_backend_users"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(BackendUsers::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(BackendUsers::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(key_col(BackendUsers::Username).not_null().unique_key())
                .col(key_col(BackendUsers::Email).not_null().unique_key())
                .col(ColumnDef::new(BackendUsers::FirstName).text().not_null())
                .col(ColumnDef::new(BackendUsers::LastName).text())
                .col(ColumnDef::new(BackendUsers::PasswordHash).text().not_null())
                // Portable booleans: `bool_col` stores a 0/1 integer, bound and
                // read as a normal `bool` through the query layer.
                .col(bool_col(BackendUsers::IsSuperuser).not_null().default(0))
                .col(bool_col(BackendUsers::IsActive).not_null().default(1))
                .col(ColumnDef::new(BackendUsers::CreatedAt).text().not_null())
                .col(ColumnDef::new(BackendUsers::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(BackendUsers::Table).to_owned())
            .await
    }
}
