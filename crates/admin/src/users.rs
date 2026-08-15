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

use crate::{not_found, render, render_error, AdminState, Permission, Shell};

/// Renders the edit form for a user, populated with their current overrides.
pub(crate) async fn edit_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(editor): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Response {
    let row = match sqlx::query!(
        r#"select username, first_name, last_name, email, is_superuser, permissions
           from backend_users where id::text = $1"#,
        id,
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(_) => return render_error(),
    };
    let Some(row) = row else {
        return not_found();
    };
    let overrides: HashMap<String, i64> =
        serde_json::from_value(row.permissions).unwrap_or_default();
    render(build(
        &state,
        shell,
        &editor.permissions,
        format!("/admin/users/{id}/edit"),
        full_name(&row.first_name, row.last_name.as_deref()),
        row.username,
        row.email,
        row.is_superuser,
        &overrides,
    ))
}

/// Persists the changed overrides, then redirects to the list.
pub(crate) async fn update(
    State(state): State<AdminState>,
    Extension(editor): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let row = match sqlx::query!(
        "select id, is_superuser, permissions from backend_users where id::text = $1",
        id,
    )
    .fetch_optional(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(_) => return render_error(),
    };
    let Some(row) = row else {
        return not_found();
    };
    // A superuser has no editable overrides; nothing to save.
    if row.is_superuser {
        return Redirect::to("/admin/users").into_response();
    }

    let mut overrides: HashMap<String, i64> =
        serde_json::from_value(row.permissions).unwrap_or_default();
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

    match state.auth.set_user_permissions(row.id, &overrides).await {
        Ok(()) => Redirect::to("/admin/users").into_response(),
        Err(_) => render_error(),
    }
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
            label: permission.label.clone(),
            state,
            changeable: editor.allows(&permission.code),
        };
        match groups.iter_mut().find(|g| g.name == permission.group) {
            Some(group) => group.rows.push(row),
            None => groups.push(PermGroupView {
                name: permission.group.clone(),
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
    overrides: &HashMap<String, i64>,
) -> UsersFormTemplate {
    let groups = if is_superuser {
        Vec::new()
    } else {
        group_permissions(&state.permissions, overrides, editor)
    };
    UsersFormTemplate {
        shell,
        action,
        cancel_path: "/admin/users".to_string(),
        full_name,
        username,
        email,
        is_superuser,
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
    groups: Vec<PermGroupView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Vec<Permission> {
        vec![
            Permission {
                code: "backend.manage_users".to_string(),
                label: "Manage backend users".to_string(),
                group: "Backend".to_string(),
            },
            Permission {
                code: "acme.publish".to_string(),
                label: "Publish".to_string(),
                group: "Content".to_string(),
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
        let groups = group_permissions(&registry(), &overrides, &editor);

        let backend = &groups[0].rows[0];
        assert_eq!(backend.state, 0);
        assert!(backend.changeable);

        let content = &groups[1].rows[0];
        assert_eq!(content.state, -1);
        // The editor does not hold acme.publish, so it is locked.
        assert!(!content.changeable);
    }
}
