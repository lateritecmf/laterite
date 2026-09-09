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

use crate::persist::{delete, DeleteError, DeleteRequest, Persister};
use crate::session::{FlashLevel, SessionHandle};
use crate::AdminState;

/// What a list needs to delete from: where to go back to, and the write path.
pub(crate) struct BulkContext {
    pub base_path: String,
    pub entity: String,
    pub persister: Arc<dyn Persister>,
    pub listeners: Vec<Arc<dyn ModelListener>>,
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
pub(crate) async fn delete_handler(
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    ctx: Arc<BulkContext>,
    headers: axum::http::HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let ids = selected(&pairs);
    let back = format!("{}{}", state.admin_path, ctx.base_path);
    if ids.is_empty() {
        session.push_flash(FlashLevel::Error, t!("Select a record first."));
        return crate::form::saved_response(crate::form::is_htmx(&headers), &back);
    }

    let actor = Actor::from(&user);
    let outcome = delete_selected(&state, &ctx, &ids, &actor).await;

    if outcome.deleted > 0 {
        session.push_flash(
            FlashLevel::Success,
            laterite_core::tn!(
                "Deleted {n} record.",
                "Deleted {n} records.",
                n = outcome.deleted as i64
            ),
        );
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
