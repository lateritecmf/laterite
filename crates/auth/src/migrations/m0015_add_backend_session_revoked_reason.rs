//! Add `revoked_reason` to `backend_sessions`.

use laterite_core::strata::*;

use crate::schema::BackendSessions;

/// Why a session was ended on purpose, kept until it is next presented so the
/// person holding it can be told.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0015_add_backend_session_revoked_reason"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .add_column(key_col(BackendSessions::RevokedReason))
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendSessions::Table)
                .drop_column(BackendSessions::RevokedReason)
                .to_owned(),
        )
        .await
    }
}
