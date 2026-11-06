//! Acting on selected list rows.
//!
//! A list marked `deletable` renders a checkbox per row and a Delete button. The
//! selection posts here, and each id goes through the same delete pipeline a
//! single record would: the transaction is owned by the pipeline, listeners run
//! inside it and may refuse, and the audit entry is written by the same listener
//! that records every other write.
//!
//! Each record is deleted in its own transaction rather than the whole selection
//! in one. A refusal then removes what it refuses and nothing else, so one
//! referenced row does not silently block the rest of the operator's selection.
//! The response says exactly what happened to each.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use axum::{Extension, Form};
use laterite_auth::AuthenticatedUser;
use laterite_core::{t, Actor, ModelListener, Text};

use crate::persist::{delete, save, DeleteError, DeleteRequest, Persister, SaveError, SaveRequest};
use crate::session::{FlashLevel, SessionHandle};
use crate::AdminState;

/// What a list needs to delete from: where to go back to, and the write path.
pub(crate) struct BulkContext {
    pub base_path: String,
    pub entity: String,
    pub persister: Arc<dyn Persister>,
    pub listeners: Vec<Arc<dyn ModelListener>>,
    /// Delete moves a record to the trash, and Restore and Delete permanently
    /// are offered in the trash view.
    pub soft_delete: bool,
    /// The listeners a trash or restore saves through: the form's own stamps
    /// (`timestamps`, `footprints`) and the contributed ones.
    pub save_listeners: Vec<Arc<dyn ModelListener>>,
}

/// What a bulk action does to each selected record.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Removes it, or on a soft-deleting resource moves it to the trash.
    Delete,
    /// Takes a trashed record out of the trash.
    Restore,
    /// Removes a trashed record for good.
    Purge,
}

/// What became of a selection.
#[derive(Default)]
pub(crate) struct Outcome {
    pub deleted: usize,
    /// One message per record a listener or persister refused, in selection
    /// order, so the operator learns why rather than that "some" failed.
    pub refused: Vec<Text>,
    pub failed: usize,
}

/// Deletes each selected id, collecting what happened.
pub(crate) async fn delete_selected(
    state: &AdminState,
    ctx: &BulkContext,
    ids: &[String],
    actor: &Actor,
) -> Outcome {
    let mut outcome = Outcome::default();
    for id in ids {
        let req = DeleteRequest {
            db: &state.db,
            cache: Some(&state.cache),
            listeners: &ctx.listeners,
            persister: ctx.persister.as_ref(),
            actor,
            entity: &ctx.entity,
            id,
        };
        match delete(req).await {
            Ok(_) => outcome.deleted += 1,
            Err(DeleteError::Refused(message)) => outcome.refused.push(message),
            Err(DeleteError::Failed(e)) => {
                tracing::error!(entity = %ctx.entity, id = %id, error = %e, "delete failed");
                outcome.failed += 1;
            }
        }
    }
    outcome
}

/// Sets or clears `deleted_at` on each selected id through the save pipeline,
/// so the listeners, the audit entry and any trash cascade run.
async fn stamp_selected(
    state: &AdminState,
    ctx: &BulkContext,
    ids: &[String],
    actor: &Actor,
    trash: bool,
) -> Outcome {
    let mut outcome = Outcome::default();
    let stamp = chrono::Utc::now();
    for id in ids {
        let Ok(numeric) = id.parse::<i64>() else {
            outcome.failed += 1;
            continue;
        };
        let mut rec = laterite_core::Record::with_id(&ctx.entity, numeric);
        if trash {
            rec.set(laterite_core::scope::DELETED_AT, stamp);
        } else {
            rec.set(
                laterite_core::scope::DELETED_AT,
                laterite_core::AttrValue::Null,
            );
        }
        let req = SaveRequest {
            db: &state.db,
            cache: Some(&state.cache),
            listeners: &ctx.save_listeners,
            persister: ctx.persister.as_ref(),
            actor,
            op: laterite_core::Op::Update,
            id: Some(id),
            then: None,
        };
        match save(req, &mut rec).await {
            Ok(()) => outcome.deleted += 1,
            Err(SaveError::Invalid(bag)) => {
                outcome.refused.extend(
                    bag.iter()
                        .flat_map(|(_, messages)| messages.iter().cloned()),
                );
            }
            Err(SaveError::Failed(e)) => {
                tracing::error!(entity = %ctx.entity, id = %id, error = %e, "trash or restore failed");
                outcome.failed += 1;
            }
        }
    }
    outcome
}

/// The selected ids that are in the trash, in selection order: Delete
/// permanently acts on nothing else.
async fn trashed_only(state: &AdminState, entity: &str, ids: &[String]) -> Vec<String> {
    use laterite_core::query::{bind_values, build, text_cast};
    use laterite_core::AnyRowExt;
    use sea_query::{Alias, Expr, Query};
    let (sql, values) = {
        let mut select = Query::select();
        select
            .expr_as(
                Expr::col(Alias::new("id")).cast_as(Alias::new(text_cast(state.db.backend))),
                Alias::new("lat_id"),
            )
            .from(Alias::new(entity))
            .and_where(Expr::col(Alias::new(laterite_core::scope::DELETED_AT)).is_not_null())
            .and_where(
                Expr::expr(
                    Expr::col(Alias::new("id")).cast_as(Alias::new(text_cast(state.db.backend))),
                )
                .is_in(ids.iter().cloned()),
            );
        build(state.db.backend, select)
    };
    let found: Vec<String> = match bind_values(sqlx::query(&sql), values)
        .fetch_all(&state.db.pool)
        .await
    {
        Ok(rows) => rows
            .iter()
            .filter_map(|r| r.get_text("lat_id").ok())
            .collect(),
        Err(e) => {
            tracing::error!(entity, error = %e, "reading the trash failed");
            Vec::new()
        }
    };
    ids.iter()
        .filter(|id| found.contains(id))
        .cloned()
        .collect()
}

/// Pulls the selected ids out of the submitted pairs. A checkbox list arrives as
/// one `id` pair per ticked box, which a map-shaped extractor would collapse to
/// the last one.
fn selected(pairs: &[(String, String)]) -> Vec<String> {
    pairs
        .iter()
        .filter(|(k, _)| k == "id")
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Handles the Delete button: removes the selection, then returns to the list
/// with a flash saying what happened.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn delete_handler(
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    ctx: Arc<BulkContext>,
    action: Action,
    headers: axum::http::HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let ids = selected(&pairs);
    // The base path is already under the admin mount; the trash view's
    // actions return to the trash.
    let back = match action {
        Action::Delete => ctx.base_path.clone(),
        Action::Restore | Action::Purge => format!("{}?trashed=1", ctx.base_path),
    };
    if ids.is_empty() {
        session.push_flash(FlashLevel::Error, t!("Select a record first."));
        return crate::form::saved_response(crate::form::is_htmx(&headers), &back);
    }

    let actor = Actor::from(&user);
    let outcome = match action {
        Action::Delete if ctx.soft_delete => stamp_selected(&state, &ctx, &ids, &actor, true).await,
        Action::Delete => delete_selected(&state, &ctx, &ids, &actor).await,
        Action::Restore => stamp_selected(&state, &ctx, &ids, &actor, false).await,
        Action::Purge => {
            let trashed = trashed_only(&state, &ctx.entity, &ids).await;
            delete_selected(&state, &ctx, &trashed, &actor).await
        }
    };

    if outcome.deleted > 0 {
        let n = outcome.deleted as i64;
        let message = match action {
            Action::Delete if ctx.soft_delete => laterite_core::tn!(
                "Moved {n} record to the trash.",
                "Moved {n} records to the trash.",
                n = n
            ),
            Action::Restore => {
                laterite_core::tn!("Restored {n} record.", "Restored {n} records.", n = n)
            }
            Action::Delete | Action::Purge => {
                laterite_core::tn!("Deleted {n} record.", "Deleted {n} records.", n = n)
            }
        };
        session.push_flash(FlashLevel::Success, message);
    }
    // Each refusal carries its own reason, so they are shown rather than counted.
    for message in outcome.refused {
        session.push_flash(FlashLevel::Error, message);
    }
    if outcome.failed > 0 {
        session.push_flash(FlashLevel::Error, t!("Some records could not be deleted."));
    }
    crate::form::saved_response(crate::form::is_htmx(&headers), &back)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_reads_every_ticked_box() {
        let pairs = vec![
            ("csrf_token".to_string(), "t".to_string()),
            ("id".to_string(), "1".to_string()),
            ("id".to_string(), "2".to_string()),
            ("id".to_string(), "  ".to_string()),
        ];
        // A map-shaped extractor would keep only the last id; blanks are dropped.
        assert_eq!(selected(&pairs), ["1", "2"]);
    }

    #[test]
    fn nothing_ticked_selects_nothing() {
        let pairs = vec![("csrf_token".to_string(), "t".to_string())];
        assert!(selected(&pairs).is_empty());
    }
}
