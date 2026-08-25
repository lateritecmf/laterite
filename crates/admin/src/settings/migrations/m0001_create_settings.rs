//! Create the settings store table.

use laterite_core::strata::*;

use crate::settings::store::Settings;

/// Creates the settings store table.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0001_create_settings"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        // `value` holds a settings model serialised as JSON text (portable JSON),
        // keyed by a stable code.
        s.exec(
            Table::create()
                .table(Settings::Table)
                .if_not_exists()
                .col(key_col(Settings::Code).not_null().primary_key())
                .col(
                    ColumnDef::new(Settings::Value)
                        .text()
                        .not_null()
                        .default("{}"),
                )
                .col(ColumnDef::new(Settings::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(Table::drop().table(Settings::Table).to_owned())
            .await
    }
}
