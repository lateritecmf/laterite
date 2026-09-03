//! Add the `locale` column to `backend_users`.

use laterite_core::strata::*;

use crate::schema::BackendUsers;

/// Adds the nullable `locale` column to `backend_users`.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0009_add_backend_user_locale"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(ColumnDef::new(BackendUsers::Locale).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::Locale)
                .to_owned(),
        )
        .await
    }
}
