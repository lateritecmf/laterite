//! Create the parameters table.

use crate::parameters::ParametersTable;
use crate::strata::*;

/// Creates the parameters table: one JSON value per `namespace::group.item`.
pub struct Migration;

#[async_trait(?Send)]
impl crate::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_parameters"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(ParametersTable::Table)
                .if_not_exists()
                .col(key_col(ParametersTable::Namespace).not_null())
                .col(key_col(ParametersTable::Group).not_null())
                .col(key_col(ParametersTable::Item).not_null())
                .col(
                    ColumnDef::new(ParametersTable::Value)
                        .text()
                        .not_null()
                        .default("null"),
                )
                .col(ColumnDef::new(ParametersTable::UpdatedAt).text().not_null())
                .primary_key(
                    Index::create()
                        .col(ParametersTable::Namespace)
                        .col(ParametersTable::Group)
                        .col(ParametersTable::Item),
                )
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(ParametersTable::Table).to_owned())
            .await
    }
}
