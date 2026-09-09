//! The audit writer as a model listener.
//!
//! Auditing applies to every entity, so this runs on all of them and needs no
//! per-entity wiring: a screen built from descriptors is audited by existing.
//! Screens that write through their own store functions rather than the save
//! pipeline still call [`crate::AuthService::record_audit`] directly.

use laterite_core::{ModelListener, Op, Record, SavedCx};

use crate::{store, AuthError};

/// Appends one entry per committed write, attributed to the save's actor.
pub struct AuditListener;

impl AuditListener {
    async fn write(&self, cx: &SavedCx<'_>, rec: &Record, verb: &str) -> Result<(), AuthError> {
        let target_id = rec.id().map(|id| id.to_string());
        store::insert_audit_log(
            cx.db(),
            cx.actor().user_id(),
            cx.actor().label(),
            &format!("backend.{}.{verb}", rec.entity()),
            Some(rec.entity()),
            target_id.as_deref(),
            None,
        )
        .await
    }
}

#[laterite_core::strata::async_trait]
impl ModelListener for AuditListener {
    /// Runs after the commit, so a failure here cannot undo the change it
    /// records: the log line is the signal to investigate, not a reason to tell
    /// the operator their save failed.
    async fn after_save(&self, cx: &SavedCx<'_>, rec: &Record, op: Op) {
        let verb = match op {
            Op::Create => "create",
            Op::Update => "update",
            // `Op` is non-exhaustive; an unnamed stage is still worth recording.
            _ => "write",
        };
        if let Err(e) = self.write(cx, rec, verb).await {
            tracing::error!(entity = rec.entity(), error = %e, "failed to write audit log entry");
        }
    }

    /// A delete is a change to the record of what happened, so it is logged like
    /// any other write. The row is already gone, which is exactly why the entry
    /// matters.
    async fn after_delete(&self, cx: &SavedCx<'_>, rec: &Record) {
        if let Err(e) = self.write(cx, rec, "delete").await {
            tracing::error!(entity = rec.entity(), error = %e, "failed to write audit log entry");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterite_core::testing::connect_test;
    use laterite_core::{Actor, Db, ModelListener};

    async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
        connect_test(&[crate::migrations()]).await
    }

    /// A real operator row: the audit entry keys to it, so a fabricated id would
    /// fail the foreign key and the listener would swallow the error.
    async fn operator(db: &Db) -> i64 {
        let svc = crate::AuthService::new(db.clone(), crate::AuthConfig::default());
        svc.create_superuser(crate::NewOperator {
            username: "root",
            email: "root@acme.test",
            first_name: "Root",
            last_name: None,
            password: "rootpw12345",
            timezone: None,
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_committed_write_is_recorded_against_its_operator() {
        let (db, _guard) = test_db().await;
        let id = operator(&db).await;
        let actor = Actor::user(id, "root");
        let mut rec = laterite_core::Record::with_id("widgets", 42);
        rec.set("name", "Chair");

        AuditListener
            .after_save(&SavedCx::new(&db, &actor), &rec, Op::Update)
            .await;

        let entries = store::recent_audit(&db, 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "backend.widgets.update");
        assert_eq!(entries[0].target_type.as_deref(), Some("widgets"));
        assert_eq!(entries[0].target_id.as_deref(), Some("42"));
        assert_eq!(entries[0].actor_user_id, Some(id));
        assert_eq!(entries[0].actor_username, "root");
    }

    #[tokio::test]
    async fn a_system_write_records_the_process_and_no_user() {
        let (db, _guard) = test_db().await;
        let actor = Actor::system("seed");
        let mut rec = laterite_core::Record::new("widgets");
        rec.set_id(1);

        AuditListener
            .after_save(&SavedCx::new(&db, &actor), &rec, Op::Create)
            .await;

        let entries = store::recent_audit(&db, 10).await.unwrap();
        assert_eq!(entries[0].action, "backend.widgets.create");
        // No foreign key to a user, and the process names itself in the trail.
        assert_eq!(entries[0].actor_user_id, None);
        assert_eq!(entries[0].actor_username, "seed");
    }
}
