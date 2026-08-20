//! The settings schema, as a portable migration.

use laterite_core::strata::*;

use super::store::Settings;

/// The stable migration namespace for the settings store.
pub const MODULE_ID: &str = "laterite.settings";

/// The settings migrations, for registration with the application's runner.
pub fn migrations() -> MigrationSet {
    MigrationSet::new(MODULE_ID, vec![Box::new(CreateSettings)])
}

struct CreateSettings;

#[async_trait(?Send)]
impl Migration for CreateSettings {
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
