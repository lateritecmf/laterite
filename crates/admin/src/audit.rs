//! Recording admin mutations in the framework audit log.
//!
//! Every admin path that affects privileges or data calls [`record`] after a
//! successful change, attributing it to the acting operator. Self-service
//! preference changes (an operator's own timezone or language) are not audited.

use laterite_auth::{AuditEntry, AuthenticatedUser};

use crate::AdminState;

/// Appends one audit entry for a mutation `user` just performed. A failed write
/// is logged but never fails the operation it records: the change has already
/// committed, so the error line is the signal to investigate, not a reason to
/// report failure to the operator.
pub(crate) async fn record(
    state: &AdminState,
    user: &AuthenticatedUser,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    detail: Option<&str>,
) {
    let entry = AuditEntry {
        actor_user_id: Some(user.user.id),
        actor_username: &user.user.username,
        action,
        target_type,
        target_id,
        detail,
    };
    if let Err(e) = state.auth.record_audit(entry).await {
        tracing::error!(action, error = %e, "failed to write audit log entry");
    }
}

/// A throwaway authenticated identity for tests that must supply an actor to a
/// mutation handler but do not exercise the audit log itself.
#[cfg(test)]
pub(crate) fn test_actor() -> AuthenticatedUser {
    use laterite_auth::{BackendUser, PermissionSet};
    AuthenticatedUser {
        user: BackendUser {
            id: 1,
            username: "tester".to_string(),
            email: "tester@example.test".to_string(),
            first_name: "Test".to_string(),
            last_name: None,
            password_hash: String::new(),
            is_superuser: true,
            is_active: true,
            timezone: None,
            locale: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
        permissions: PermissionSet::default(),
    }
}
