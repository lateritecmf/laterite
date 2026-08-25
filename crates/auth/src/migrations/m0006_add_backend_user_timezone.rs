//! Add the `timezone` column to `backend_users`.

use laterite_core::strata::*;

use crate::schema::BackendUsers;

/// Adds the nullable `timezone` column to `backend_users`.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0006_add_backend_user_timezone"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .add_column(ColumnDef::new(BackendUsers::Timezone).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendUsers::Table)
                .drop_column(BackendUsers::Timezone)
                .to_owned(),
        )
        .await
    }
}
