//! Acting on selected list rows.
//!
//! A list names its own actions under `bulk_actions`: each is a button, or an
//! entry of a dropdown, that stays disabled until a row is ticked and then
//! posts the ticked ids. An action either sets fields on each record (`set`)
//! or hands the ids to a registered [`BulkAction`] (`handler`).
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
use axum::response::{IntoResponse, Response};
use axum::{Extension, Form};
use laterite_auth::AuthenticatedUser;
use laterite_core::{t, Actor, Db, ModelListener, Text};

use crate::persist::{delete, save, DeleteError, DeleteRequest, Persister, SaveError, SaveRequest};
use crate::session::{FlashLevel, SessionHandle};
use crate::AdminState;

/// What a registered bulk action knows about the request.
#[non_exhaustive]
pub struct BulkCx<'a> {
    /// The database.
    pub db: &'a Db,
    /// The table the list shows.
    pub entity: &'a str,
    /// Who is acting, for a write through the save pipeline.
    pub actor: &'a Actor,
    /// The signed-in user and what they may do.
    pub user: &'a AuthenticatedUser,
}

/// What a bulk action did.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct BulkOutcome {
    /// How many records it acted on.
    pub done: usize,
    /// One message per record it refused, shown as they are.
    pub refused: Vec<Text>,
    /// What to say in place of the list's own message.
    pub message: Option<Text>,
    /// Where to go in place of the list: a path under the admin root.
    pub redirect: Option<String>,
}

impl BulkOutcome {
    /// An outcome over `done` records.
    pub fn done(done: usize) -> Self {
        Self {
            done,
            ..Self::default()
        }
    }

    /// Adds a record's refusal.
    pub fn refuse(mut self, message: impl Into<Text>) -> Self {
        self.refused.push(message.into());
        self
    }

    /// Says `message` in place of the list's own.
    pub fn message(mut self, message: impl Into<Text>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Goes to `path` (under the admin root) in place of the list.
    pub fn redirect(mut self, path: impl Into<String>) -> Self {
        self.redirect = Some(path.into());
        self
    }
}

/// A bulk action failed as a whole.
#[derive(Debug, thiserror::Error)]
#[error("bulk action error: {0}")]
pub struct BulkError(pub String);

/// An action over the rows ticked in a list.
#[laterite_core::strata::async_trait]
pub trait BulkAction: Send + Sync + 'static {
    /// Acts on `ids`, the ticked records' ids in the order shown.
    async fn run(&self, cx: &BulkCx<'_>, ids: &[String]) -> Result<BulkOutcome, BulkError>;
}

/// A registered bulk action under its dotted `vendor.name` key.
pub struct BulkActionReg {
    /// The action's dotted `vendor.name` key.
    pub name: String,
    /// The action being registered.
    pub action: Arc<dyn BulkAction>,
}

impl BulkActionReg {
    /// Registers `action` as `name`.
    pub fn new(name: impl Into<String>, action: Arc<dyn BulkAction>) -> Self {
        Self {
            name: name.into(),
            action,
        }
    }
}

/// The bulk-action registry, keyed by action name.
pub(crate) type BulkRegistry = std::collections::HashMap<String, BulkActionReg>;

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
    /// The resource's form, which a bulk action's `set` saves through so its
    /// rules run. A resource with no form writes the columns directly.
    pub form: Option<Arc<crate::form::PreparedForm>>,
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
    db: &Db,
    cache: &laterite_core::Cache,
    ctx: &BulkContext,
    ids: &[String],
    actor: &Actor,
) -> Outcome {
    let mut outcome = Outcome::default();
    for id in ids {
        let req = DeleteRequest {
            db,
            cache: Some(cache),
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
    db: &Db,
    cache: &laterite_core::Cache,
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
            db,
            cache: Some(cache),
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

/// Sets `fields` on each selected id through the save pipeline, so the
/// listeners and the audit entry run for every record.
async fn set_selected(
    db: &Db,
    cache: &laterite_core::Cache,
    ctx: &BulkContext,
    ids: &[String],
    actor: &Actor,
    fields: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Outcome {
    use laterite_core::AttrValue;
    let mut outcome = Outcome::default();
    for id in ids {
        let Ok(numeric) = id.parse::<i64>() else {
            outcome.failed += 1;
            continue;
        };
        // Through the form when there is one: the record as its edit page
        // holds it, with the action's fields laid over, validated and saved.
        if let Some(form) = &ctx.form {
            let handle = crate::form::FormHandle::new(db.clone(), cache.clone(), form.clone());
            let saved = match handle.load(numeric).await {
                Ok(Some(mut data)) => {
                    for (field, value) in fields {
                        let text = match value {
                            serde_json::Value::Null | serde_json::Value::Bool(false) => {
                                String::new()
                            }
                            serde_json::Value::Bool(true) => "1".to_string(),
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        data.insert(field.clone(), text);
                    }
                    handle.update(numeric, data, actor).await
                }
                Ok(None) => continue,
                Err(e) => Err(e),
            };
            match saved {
                Ok(()) => outcome.deleted += 1,
                Err(SaveError::Invalid(bag)) => {
                    outcome.refused.extend(
                        bag.iter()
                            .flat_map(|(_, messages)| messages.iter().cloned()),
                    );
                }
                Err(SaveError::Failed(e)) => {
                    tracing::error!(entity = %ctx.entity, id = %id, error = %e, "a bulk update failed");
                    outcome.failed += 1;
                }
            }
            continue;
        }
        let mut rec = laterite_core::Record::with_id(&ctx.entity, numeric);
        for (field, value) in fields {
            let value = match value {
                serde_json::Value::Null => AttrValue::Null,
                serde_json::Value::Bool(b) => AttrValue::Bool(*b),
                serde_json::Value::String(s) => AttrValue::Text(s.clone()),
                serde_json::Value::Number(n) => match n.as_i64() {
                    Some(i) => AttrValue::Int(i),
                    None => AttrValue::Float(n.as_f64().unwrap_or_default()),
                },
                other => AttrValue::Json(other.clone()),
            };
            rec.set(field, value);
        }
        let req = SaveRequest {
            db,
            cache: Some(cache),
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
                tracing::error!(entity = %ctx.entity, id = %id, error = %e, "a bulk update failed");
                outcome.failed += 1;
            }
        }
    }
    outcome
}

/// Handles one of a list's own bulk actions: sets its fields on the ticked
/// records, or hands them to its registered handler, then returns to the list
/// saying what happened.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn action_handler(
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    ctx: Arc<BulkContext>,
    spec: Arc<crate::list::BulkActionSpec>,
    headers: axum::http::HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    if !spec
        .permission
        .as_deref()
        .is_none_or(|p| user.permissions.allows(p))
    {
        return crate::AdminError::Forbidden.into_response();
    }
    let ids = selected(&pairs);
    let mut back = if spec.trash {
        format!("{}?trashed=1", ctx.base_path)
    } else {
        ctx.base_path.clone()
    };
    let htmx = crate::form::is_htmx(&headers);
    if ids.is_empty() {
        session.push_flash(FlashLevel::Error, t!("Select a record first."));
        return crate::form::saved_response(htmx, &back);
    }
    let actor = Actor::from(&user);
    let (done, refused, failed, message) = match &spec.handler {
        Some(name) => {
            let Some(reg) = state.bulk_actions.get(name) else {
                return crate::AdminError::NotFound.into_response();
            };
            let cx = BulkCx {
                db: &state.db,
                entity: &ctx.entity,
                actor: &actor,
                user: &user,
            };
            match reg.action.run(&cx, &ids).await {
                Ok(outcome) => {
                    if let Some(path) = outcome.redirect {
                        back = format!("{}{path}", state.admin_path);
                    }
                    (outcome.done, outcome.refused, 0, outcome.message)
                }
                Err(e) => {
                    tracing::error!(action = %name, entity = %ctx.entity, error = %e, "a bulk action failed");
                    (0, Vec::new(), ids.len(), None)
                }
            }
        }
        None => {
            let outcome =
                set_selected(&state.db, &state.cache, &ctx, &ids, &actor, &spec.set).await;
            (outcome.deleted, outcome.refused, outcome.failed, None)
        }
    };
    if let Some(message) = message {
        session.push_flash(FlashLevel::Success, message);
    } else if done > 0 {
        let n = done as i64;
        let message = match &spec.done {
            Some(text) => text.clone().arg("n", n),
            None => laterite_core::tn!("Updated {n} record.", "Updated {n} records.", n = n),
        };
        session.push_flash(FlashLevel::Success, message);
    }
    for message in refused {
        session.push_flash(FlashLevel::Error, message);
    }
    if failed > 0 {
        session.push_flash(FlashLevel::Error, t!("Some records could not be changed."));
    }
    crate::form::saved_response(htmx, &back)
}

/// Removes one record as the list's Delete would: to the trash on a
/// soft-deleting resource, for good otherwise.
pub(crate) async fn remove(
    db: &Db,
    cache: &laterite_core::Cache,
    ctx: &BulkContext,
    id: &str,
    actor: &Actor,
) -> Outcome {
    let ids = [id.to_string()];
    if ctx.soft_delete {
        stamp_selected(db, cache, ctx, &ids, actor, true).await
    } else {
        delete_selected(db, cache, ctx, &ids, actor).await
    }
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
/// Removes every row of a log's table, for a list that is `clearable`. Not a
/// record-by-record delete: no listener runs, which is what makes it only a
/// log's to offer. Audited with the number of rows that went.
pub(crate) async fn clear_handler(
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    entity: Arc<str>,
    back: Arc<str>,
    permission: Option<Arc<str>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if permission
        .as_deref()
        .is_some_and(|p| !user.permissions.allows(p))
    {
        return axum::response::IntoResponse::into_response(axum::http::StatusCode::FORBIDDEN);
    }
    if !crate::sql::valid_ident(&entity) {
        return crate::render_error();
    }
    let (sql, values) = laterite_core::query::build(
        state.db.backend,
        sea_query::Query::delete()
            .from_table(sea_query::Alias::new(&*entity))
            .to_owned(),
    );
    let removed = match laterite_core::query::bind_values(sqlx::query(&sql), values)
        .execute(&state.db.pool)
        .await
    {
        Ok(done) => done.rows_affected(),
        Err(error) => {
            tracing::error!(%error, entity = %entity, "clearing a log failed");
            session.push_flash(FlashLevel::Error, t!("The log could not be cleared."));
            return crate::form::saved_response(crate::form::is_htmx(&headers), &back);
        }
    };
    crate::audit::record(
        &state,
        &user,
        &format!("backend.{entity}.clear"),
        Some(&entity),
        None,
        None,
        Some(&format!("{removed} rows")),
    )
    .await;
    session.push_flash(FlashLevel::Success, t!("Log cleared."));
    crate::form::saved_response(crate::form::is_htmx(&headers), &back)
}

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
        Action::Delete if ctx.soft_delete => {
            stamp_selected(&state.db, &state.cache, &ctx, &ids, &actor, true).await
        }
        Action::Delete => delete_selected(&state.db, &state.cache, &ctx, &ids, &actor).await,
        Action::Restore => stamp_selected(&state.db, &state.cache, &ctx, &ids, &actor, false).await,
        Action::Purge => {
            let trashed = trashed_only(&state, &ctx.entity, &ids).await;
            delete_selected(&state.db, &state.cache, &ctx, &trashed, &actor).await
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
