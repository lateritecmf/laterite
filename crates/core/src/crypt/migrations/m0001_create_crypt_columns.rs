//! Create the registry of sealed columns.

use crate::crypt::CryptColumns;
use crate::strata::*;

/// Creates the table naming every column that holds sealed values, so a key
/// rotation knows what to re-seal.
pub struct Migration;

#[async_trait(?Send)]
impl crate::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_crypt_columns"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(CryptColumns::Table)
                .if_not_exists()
                .col(key_col(CryptColumns::TableName).not_null())
                .col(key_col(CryptColumns::ColumnName).not_null())
                .col(key_col(CryptColumns::Purpose).not_null())
                .col(ColumnDef::new(CryptColumns::RegisteredAt).text().not_null())
                .primary_key(
                    Index::create()
                        .col(CryptColumns::TableName)
                        .col(CryptColumns::ColumnName),
                )
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(CryptColumns::Table).to_owned())
            .await
    }
}
