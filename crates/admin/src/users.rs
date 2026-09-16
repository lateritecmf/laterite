//! The backend user screen's edit form: the per-user permission override editor.
//!
//! A user's roles set a base of permissions; this screen refines them per user
//! with a three-state control per permission: **allow** (`1`) forces it on,
//! **deny** (`-1`) forces it off, and **inherit** (absent) defers to the roles.
//! Overrides take precedence over the roles (see `laterite_auth::PermissionSet`).
//!
//! Two safeguards mirror the reference system:
//!
//! - A superuser holds every permission unconditionally, so the editor shows a
//!   note instead of the controls for them.
//! - An operator may only change permissions they themselves hold. Controls for
//!   permissions they lack are shown disabled, and a save never alters those, so
//!   the screen cannot be used to escalate beyond the editor's own access.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use laterite_auth::{AuthenticatedUser, PermissionSet};
use laterite_core::query::{bind_values, build as to_sql, text_cast};
use laterite_core::{t, AnyRowExt};
use sea_query::{Alias, Expr, Query};

use crate::{render, AdminError, AdminState, Permission, Shell};

/// The permission that gates changing an account's state, the same one that
/// gates reaching this screen at all.
pub(crate) const MANAGE_USERS: &str = "backend.manage_users";

/// Activates or deactivates an account.
///
/// The service owns the rules (not yourself, not the last superuser) and drops
/// the account's sessions and credentials when it deactivates; this handler
/// carries the operator's decision to it and their refusal back.
pub(crate) async fn set_active(
    State(state): State<AdminState>,
    Extension(editor): Extension<AuthenticatedUser>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Path(id): Path<String>,
    Form(form): Form<ActiveForm>,
) -> Result<Response, AdminError> {
    editor
        .require(MANAGE_USERS)
        .map_err(|_| AdminError::Forbidden)?;
    let target: i64 = id.parse().map_err(|_| AdminError::NotFound)?;
    let active = form.active == "1";

    match state
        .auth
        .set_user_active(editor.user.id, target, active)
        .await
    {
        Ok(()) => {
            crate::audit::record(
                &state,
                &editor,
                if active {
                    "backend.user.activate"
                } else {
                    "backend.user.deactivate"
                },
                Some("backend_user"),
                Some(&id),
                None,
            )
            .await;
            session.push_flash(
                crate::session::FlashLevel::Success,
                if active {
                    t!("That account can sign in again.")
                } else {
                    t!("That account is deactivated and its sessions have ended.")
                },
            );
        }
        // A refusal is the operator's to read: it names which rule stopped them.
        Err(laterite_auth::AuthError::Refused(reason)) => {
            session.push_flash(
                crate::session::FlashLevel::Error,
                laterite_core::Text::dynamic(&reason),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "changing an account's state failed");
            session.push_flash(
                crate::session::FlashLevel::Error,
                t!("That account could not be changed."),
            );
        }
    }
    Ok(Redirect::to(&format!("{}/users/{id}/edit", state.admin_path)).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct ActiveForm {
    /// "1" to activate, anything else to deactivate.
    active: String,
}

/// Renders the edit form for a user, populated with their current overrides.
pub(crate) async fn edit_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(editor): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    // Scope the builder so it drops before the await, keeping the future `Send`.
    let (sql, values) = {
        let stmt = Query::select()
            .columns([
                Alias::new("username"),
                Alias::new("first_name"),
                Alias::new("last_name"),
                Alias::new("email"),
                Alias::new("is_superuser"),
                Alias::new("is_active"),
                Alias::new("permissions"),
            ])
            .from(Alias::new("backend_users"))
            .and_where(
                Expr::col(Alias::new("id"))
                    .cast_as(Alias::new(text_cast(state.db.backend)))
                    .eq(id.clone()),
            )
            .to_owned();
        to_sql(state.db.backend, stmt)
    };
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&state.db.pool)
        .await?
        .ok_or(AdminError::NotFound)?;
    let username = row.get_text("username").unwrap_or_default();
    let first_name = row.get_text("first_name").unwrap_or_default();
    let last_name = row.get_text_opt("last_name").unwrap_or_default();
    let email = row.get_text("email").unwrap_or_default();
    let is_superuser = row.get_bool("is_superuser").unwrap_or(false);
    let is_active = row.get_bool("is_active").unwrap_or(true);
    // Refused server-side either way; hiding the control keeps the screen from
    // offering an action it will not carry out.
    let can_change_state =
        editor.user.id.to_string() != id && editor.permissions.allows(crate::users::MANAGE_USERS);
    let perms_json = row.get_text("permissions").unwrap_or_default();
    let overrides: HashMap<String, i64> = serde_json::from_str(&perms_json).unwrap_or_default();
    Ok(render(build(
        &state,
        shell,
        &editor.permissions,
        format!("{}/users/{id}/edit", state.admin_path),
        full_name(&first_name, last_name.as_deref()),
        username,
        email,
        is_superuser,
        is_active,
        can_change_state,
        format!("{}/users/{id}/active", state.admin_path),
        &overrides,
    )))
}

/// Persists the changed overrides, then redirects to the list.
pub(crate) async fn update(
    State(state): State<AdminState>,
    Extension(editor): Extension<AuthenticatedUser>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Path(id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AdminError> {
    // Scope the builder so it drops before the await, keeping the future `Send`.
    let (sql, values) = {
        let stmt = Query::select()
            .columns([Alias::new("is_superuser"), Alias::new("permissions")])
            .from(Alias::new("backend_users"))
            .and_where(
                Expr::col(Alias::new("id"))
                    .cast_as(Alias::new(text_cast(state.db.backend)))
                    .eq(id.clone()),
            )
            .to_owned();
        to_sql(state.db.backend, stmt)
    };
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&state.db.pool)
        .await?
        .ok_or(AdminError::NotFound)?;
    // A superuser has no editable overrides; nothing to save.
    if row.get_bool("is_superuser").unwrap_or(false) {
        return Ok(Redirect::to(&format!("{}/users/{id}/edit", state.admin_path)).into_response());
    }
    let target_id = id.parse::<i64>().map_err(|_| AdminError::NotFound)?;

    let perms_json = row.get_text("permissions").unwrap_or_default();
    let mut overrides: HashMap<String, i64> = serde_json::from_str(&perms_json).unwrap_or_default();
    let submitted = parse_states(&pairs);

    // Only touch registered permissions the editor holds. A permission the editor
    // cannot grant is left exactly as it was, so the screen cannot escalate
    // access beyond the editor's own.
    for permission in state.permissions.iter() {
        if !editor.allows(&permission.code) {
            continue;
        }
        match submitted.get(&permission.code).copied() {
            Some(1) => {
                overrides.insert(permission.code.clone(), 1);
            }
            Some(-1) => {
                overrides.insert(permission.code.clone(), -1);
            }
            Some(_) => {
                overrides.remove(&permission.code);
            }
            None => {}
        }
    }

    state
        .auth
        .set_user_permissions(target_id, &overrides)
        .await?;
    let detail = serde_json::to_string(&overrides).unwrap_or_default();
    crate::audit::record(
        &state,
        &editor,
        "backend.user.permissions.update",
        Some("backend_user"),
        Some(id.as_str()),
        Some(detail.as_str()),
    )
    .await;
    session.push_flash(
        crate::session::FlashLevel::Success,
        t!("Permissions updated."),
    );
    // Back to the operator being edited, not the list. Saving is not leaving.
    Ok(Redirect::to(&format!("{}/users/{id}/edit", state.admin_path)).into_response())
}

/// Pulls the submitted permission states out of the form. Each control posts one
/// `p:<code>` pair with value `1`, `0`, or `-1`.
fn parse_states(pairs: &[(String, String)]) -> HashMap<String, i64> {
    let mut states = HashMap::new();
    for (key, value) in pairs {
        if let Some(code) = key.strip_prefix("p:") {
            if let Ok(state) = value.parse::<i64>() {
                states.insert(code.to_string(), state);
            }
        }
    }
    states
}

fn full_name(first: &str, last: Option<&str>) -> String {
    match last {
        Some(last) if !last.is_empty() => format!("{first} {last}"),
        _ => first.to_string(),
    }
}

/// Groups the registered permissions, each with the user's current state and
/// whether the editor may change it (an editor can only change permissions they
/// themselves hold).
fn group_permissions(
    registry: &[Permission],
    overrides: &HashMap<String, i64>,
    editor: &PermissionSet,
    shell: &Shell,
) -> Vec<PermGroupView> {
    let mut groups: Vec<PermGroupView> = Vec::new();
    for permission in registry {
        let state = match overrides.get(&permission.code).copied() {
            Some(1) => 1,
            Some(-1) => -1,
            _ => 0,
        };
        let row = PermRowView {
            code: permission.code.clone(),
            label: shell.tt(&permission.label),
            state,
            changeable: editor.allows(&permission.code),
        };
        // Merge by the localized group heading (same source localizes identically).
        let gname = shell.tt(&permission.group);
        match groups.iter_mut().find(|g| g.name == gname) {
            Some(group) => group.rows.push(row),
            None => groups.push(PermGroupView {
                name: gname,
                rows: vec![row],
            }),
        }
    }
    groups
}

#[allow(clippy::too_many_arguments)]
fn build(
    state: &AdminState,
    shell: Shell,
    editor: &PermissionSet,
    action: String,
    full_name: String,
    username: String,
    email: String,
    is_superuser: bool,
    is_active: bool,
    can_change_state: bool,
    state_action: String,
    overrides: &HashMap<String, i64>,
) -> UsersFormTemplate {
    let groups = if is_superuser {
        Vec::new()
    } else {
        group_permissions(&state.permissions, overrides, editor, &shell)
    };
    UsersFormTemplate {
        shell,
        action,
        cancel_path: format!("{}/users", state.admin_path),
        full_name,
        username,
        email,
        is_superuser,
        is_active,
        can_change_state,
        state_action,
        groups,
    }
}

struct PermRowView {
    code: String,
    label: String,
    state: i32,
    changeable: bool,
}

struct PermGroupView {
    name: String,
    rows: Vec<PermRowView>,
}

#[derive(Template)]
#[template(path = "users_form.html")]
struct UsersFormTemplate {
    shell: Shell,
    action: String,
    cancel_path: String,
    full_name: String,
    username: String,
    email: String,
    is_superuser: bool,
    /// Whether this account may sign in.
    is_active: bool,
    /// Whether the viewing operator may change that: not their own account, and
    /// they must hold the permission. The server refuses either way; hiding the
    /// control keeps the screen from offering what it will not do.
    can_change_state: bool,
    state_action: String,
    groups: Vec<PermGroupView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Vec<Permission> {
        vec![
            Permission {
                code: "backend.manage_users".to_string(),
                label: "Manage backend users".into(),
                group: "Backend".into(),
            },
            Permission {
                code: "acme.publish".to_string(),
                label: "Publish".into(),
                group: "Content".into(),
            },
        ]
    }

    #[test]
    fn parse_states_reads_prefixed_radio_values() {
        let pairs = vec![
            ("p:backend.manage_users".to_string(), "1".to_string()),
            ("p:acme.publish".to_string(), "-1".to_string()),
            ("other".to_string(), "ignored".to_string()),
        ];
        let states = parse_states(&pairs);
        assert_eq!(states.get("backend.manage_users"), Some(&1));
        assert_eq!(states.get("acme.publish"), Some(&-1));
        assert_eq!(states.get("other"), None);
    }

    #[test]
    fn grouping_marks_state_and_changeability() {
        // An editor who holds only backend.manage_users.
        let editor = PermissionSet::new(false, ["backend.manage_users".to_string()]);
        let overrides = HashMap::from([("acme.publish".to_string(), -1i64)]);
        let groups = group_permissions(&registry(), &overrides, &editor, &Shell::test());

        let backend = &groups[0].rows[0];
        assert_eq!(backend.state, 0);
        assert!(backend.changeable);

        let content = &groups[1].rows[0];
        assert_eq!(content.state, -1);
        // The editor does not hold acme.publish, so it is locked.
        assert!(!content.changeable);
    }
}
