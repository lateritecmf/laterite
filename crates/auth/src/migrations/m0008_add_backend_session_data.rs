//! Add the opaque `data` column to `backend_sessions`.

use laterite_core::strata::*;

use crate::schema::BackendSessions;

/// Adds the nullable `data` blob to `backend_sessions`. The surface owns its
/// contents; auth stores it as text and never queries into it.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0008_add_backend_session_data"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .add_column(ColumnDef::new(BackendSessions::Data).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .drop_column(BackendSessions::Data)
                .to_owned(),
        )
        .await
    }
}
