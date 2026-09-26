//! Add `revoked_reason` to `backend_remember_tokens`.

use laterite_core::strata::*;

use crate::schema::BackendRememberTokens;

/// Why a stay-signed-in credential was ended on purpose, so a device returning
/// on it is told, the same as one returning on a session.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {
    fn name(&self) -> &str {
        "0016_add_backend_remember_revoked_reason"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRememberTokens::Table)
                .add_column(key_col(BackendRememberTokens::RevokedReason))
                .to_owned(),
        )
        .await
    }
    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::alter()
                .table(BackendRememberTokens::Table)
                .drop_column(BackendRememberTokens::RevokedReason)
                .to_owned(),
        )
        .await
    }
}
