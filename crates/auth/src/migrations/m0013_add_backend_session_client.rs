//! Add `ip_address` and `user_agent` to `backend_sessions`.

use laterite_core::strata::*;

use crate::schema::BackendSessions;

/// Records where each session signed in from, so an account's sessions list can
/// tell one device from another. Nullable: a deployment that trusts no proxy and
/// sees no peer address has nothing to record.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0013_add_backend_session_client"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .add_column(ColumnDef::new(BackendSessions::IpAddress).text())
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .add_column(ColumnDef::new(BackendSessions::UserAgent).text())
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .drop_column(BackendSessions::IpAddress)
                .to_owned(),
        )
        .await?;
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .drop_column(BackendSessions::UserAgent)
                .to_owned(),
        )
        .await
    }
}
