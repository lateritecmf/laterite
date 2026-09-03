//! The roles screen's create and edit form: the permission editor.
//!
//! Roles carry a `text[]` of dotted permission strings. This dedicated form
//! renders the registered permissions ([`crate::Permission`]) as grouped
//! checkboxes, checked for the ones the role holds, and persists the selection.
//! Only registered permissions are accepted, so a crafted submission cannot
//! grant a permission the deployment never declared. The roles list stays a
//! generic resource; only its form is specialised.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use laterite_core::query::{bind_values, build as to_sql, text_cast};
use laterite_core::{t, AnyRowExt};
use sea_query::{Alias, Expr, Query};

use crate::{not_found, render, render_error, AdminState, Permission, Shell};

/// Renders an empty create form.
pub(crate) async fn new_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let action = format!("{}/roles/new", state.admin_path);
    render(build(&state, &action, None, "", "", &[], shell))
}

/// Persists a new role, then redirects to the list.
pub(crate) async fn create(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let (code, name, perms) = parse(&pairs);
    let perms = registered_only(perms, &state.permissions);
    let action = format!("{}/roles/new", state.admin_path);
    if code.is_empty() || name.is_empty() {
        return render(build(
            &state,
            &action,
            Some("Code and name are required."),
            &code,
            &name,
            &perms,
            shell,
        ));
    }
    match laterite_auth::store::create_role(&state.db, &code, &name, &perms).await {
        Ok(_) => {
            session.push_flash(crate::session::FlashLevel::Success, t!("Role created."));
            Redirect::to(&format!("{}/roles", state.admin_path)).into_response()
        }
        Err(_) => render(build(
            &state,
            &action,
            Some("Could not save. The code may already be in use."),
            &code,
            &name,
            &perms,
            shell,
        )),
    }
}

/// Renders the form populated with an existing role.
pub(crate) async fn edit_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path(id): Path<String>,
) -> Response {
    let action = format!("{}/roles/{id}/edit", state.admin_path);
    // Scope the builder so it drops before the await, keeping the future `Send`.
    let (sql, values) = {
        let stmt = Query::select()
            .columns([
                Alias::new("code"),
                Alias::new("name"),
                Alias::new("permissions"),
            ])
            .from(Alias::new("backend_roles"))
            .and_where(
                Expr::col(Alias::new("id"))
                    .cast_as(Alias::new(text_cast(state.db.backend)))
                    .eq(id.clone()),
            )
            .to_owned();
        to_sql(state.db.backend, stmt)
    };
    let row = match bind_values(sqlx::query(&sql), values)
        .fetch_optional(&state.db.pool)
        .await
    {
        Ok(row) => row,
        Err(_) => return render_error(),
    };
    let Some(row) = row else {
        return not_found();
    };
    let code = row.get_text("code").unwrap_or_default();
    let name = row.get_text("name").unwrap_or_default();
    let perms_json = row.get_text("permissions").unwrap_or_default();
    let perms: Vec<String> = serde_json::from_str(&perms_json).unwrap_or_default();
    render(build(&state, &action, None, &code, &name, &perms, shell))
}

/// Persists an edited role, then redirects to the list.
pub(crate) async fn update(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Path(id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let (code, name, perms) = parse(&pairs);
    let perms = registered_only(perms, &state.permissions);
    let action = format!("{}/roles/{id}/edit", state.admin_path);
    if code.is_empty() || name.is_empty() {
        return render(build(
            &state,
            &action,
            Some("Code and name are required."),
            &code,
            &name,
            &perms,
            shell,
        ));
    }
    let perms_json = serde_json::to_string(&perms).unwrap_or_else(|_| "[]".to_string());
    // Scope the builder so it drops before the await, keeping the future `Send`.
    let (sql, values) = {
        let stmt = Query::update()
            .table(Alias::new("backend_roles"))
            .value(Alias::new("code"), code.clone())
            .value(Alias::new("name"), name.clone())
            .value(Alias::new("permissions"), perms_json)
            .and_where(
                Expr::col(Alias::new("id"))
                    .cast_as(Alias::new(text_cast(state.db.backend)))
                    .eq(id.clone()),
            )
            .to_owned();
        to_sql(state.db.backend, stmt)
    };
    match bind_values(sqlx::query(&sql), values)
        .execute(&state.db.pool)
        .await
    {
        Ok(_) => {
            session.push_flash(crate::session::FlashLevel::Success, t!("Role updated."));
            Redirect::to(&format!("{}/roles", state.admin_path)).into_response()
        }
        Err(_) => render(build(
            &state,
            &action,
            Some("Could not save. The code may already be in use."),
            &code,
            &name,
            &perms,
            shell,
        )),
    }
}

/// Pulls `code`, `name`, and the repeated `perm` values out of the submitted
/// form pairs. A checkbox list arrives as one `perm` pair per ticked box, so the
/// form is decoded as an ordered list of pairs rather than a map.
fn parse(pairs: &[(String, String)]) -> (String, String, Vec<String>) {
    let mut code = String::new();
    let mut name = String::new();
    let mut perms = Vec::new();
    for (key, value) in pairs {
        match key.as_str() {
            "code" => code = value.trim().to_string(),
            "name" => name = value.trim().to_string(),
            "perm" => perms.push(value.clone()),
            _ => {}
        }
    }
    (code, name, perms)
}

/// Keeps only the submitted permissions that the deployment has registered.
fn registered_only(perms: Vec<String>, registry: &[Permission]) -> Vec<String> {
    perms
        .into_iter()
        .filter(|p| registry.iter().any(|r| &r.code == p))
        .collect()
}

/// Groups the registered permissions by their `group`, preserving registry
/// order, and marks the ones the role currently holds.
fn group_permissions(registry: &[Permission], selected: &[String]) -> Vec<PermGroupView> {
    let mut groups: Vec<PermGroupView> = Vec::new();
    for permission in registry {
        let check = PermCheckView {
            code: permission.code.clone(),
            label: permission.label.clone(),
            checked: selected.iter().any(|s| s == &permission.code),
        };
        match groups.iter_mut().find(|g| g.name == permission.group) {
            Some(group) => group.perms.push(check),
            None => groups.push(PermGroupView {
                name: permission.group.clone(),
                perms: vec![check],
            }),
        }
    }
    groups
}

#[allow(clippy::too_many_arguments)]
fn build(
    state: &AdminState,
    action: &str,
    error: Option<&str>,
    code: &str,
    name: &str,
    selected: &[String],
    shell: Shell,
) -> RolesFormTemplate {
    RolesFormTemplate {
        shell,
        title: "Role".to_string(),
        action: action.to_string(),
        cancel_path: format!("{}/roles", state.admin_path),
        error: error.map(str::to_string),
        code: code.to_string(),
        name: name.to_string(),
        groups: group_permissions(&state.permissions, selected),
    }
}

struct PermCheckView {
    code: String,
    label: String,
    checked: bool,
}

struct PermGroupView {
    name: String,
    perms: Vec<PermCheckView>,
}

#[derive(Template)]
#[template(path = "roles_form.html")]
struct RolesFormTemplate {
    shell: Shell,
    title: String,
    action: String,
    cancel_path: String,
    error: Option<String>,
    code: String,
    name: String,
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
    fn parse_pulls_scalars_and_repeated_perms() {
        let pairs = vec![
            ("code".to_string(), "  editor ".to_string()),
            ("name".to_string(), "Editor".to_string()),
            ("perm".to_string(), "backend.manage_users".to_string()),
            ("perm".to_string(), "acme.publish".to_string()),
        ];
        let (code, name, perms) = parse(&pairs);
        assert_eq!(code, "editor");
        assert_eq!(name, "Editor");
        assert_eq!(perms, ["backend.manage_users", "acme.publish"]);
    }

    #[test]
    fn only_registered_permissions_survive() {
        let kept = registered_only(
            vec![
                "backend.manage_users".to_string(),
                "nope.invalid".to_string(),
            ],
            &registry(),
        );
        assert_eq!(kept, ["backend.manage_users"]);
    }

    #[test]
    fn grouping_preserves_order_and_marks_selected() {
        let groups = group_permissions(&registry(), &["acme.publish".to_string()]);
        assert_eq!(groups[0].name, "Backend");
        assert_eq!(groups[1].name, "Content");
        assert!(!groups[0].perms[0].checked);
        assert!(groups[1].perms[0].checked);
    }
}
