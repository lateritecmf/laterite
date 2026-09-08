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
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Form};
use laterite_auth::AuthenticatedUser;
use laterite_core::query::{bind_values, build as to_sql, text_cast};
use laterite_core::{t, AnyRowExt, Text};
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
    Extension(user): Extension<AuthenticatedUser>,
    headers: axum::http::HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let htmx = crate::form::is_htmx(&headers);
    let (code, name, perms) = parse(&pairs);
    let perms = registered_only(perms, &state.permissions);
    let action = format!("{}/roles/new", state.admin_path);
    if code.is_empty() || name.is_empty() {
        return invalid_response(
            htmx,
            build(
                &state,
                &action,
                Some(t!("Code and name are required.")),
                &code,
                &name,
                &perms,
                shell,
            ),
        );
    }
    match laterite_auth::store::create_role(&state.db, &code, &name, &perms).await {
        Ok(role_id) => {
            let detail = serde_json::json!({ "code": code, "name": name }).to_string();
            let target_id = role_id.to_string();
            crate::audit::record(
                &state,
                &user,
                "backend.role.create",
                Some("backend_role"),
                Some(target_id.as_str()),
                Some(detail.as_str()),
            )
            .await;
            session.push_flash(crate::session::FlashLevel::Success, t!("Role created."));
            crate::form::saved_response(htmx, &format!("{}/roles", state.admin_path))
        }
        Err(_) => invalid_response(
            htmx,
            build(
                &state,
                &action,
                Some(t!("Could not save. The code may already be in use.")),
                &code,
                &name,
                &perms,
                shell,
            ),
        ),
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
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let htmx = crate::form::is_htmx(&headers);
    let (code, name, perms) = parse(&pairs);
    let perms = registered_only(perms, &state.permissions);
    let action = format!("{}/roles/{id}/edit", state.admin_path);
    if code.is_empty() || name.is_empty() {
        return invalid_response(
            htmx,
            build(
                &state,
                &action,
                Some(t!("Code and name are required.")),
                &code,
                &name,
                &perms,
                shell,
            ),
        );
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
            let detail = serde_json::json!({ "code": code, "name": name }).to_string();
            crate::audit::record(
                &state,
                &user,
                "backend.role.update",
                Some("backend_role"),
                Some(id.as_str()),
                Some(detail.as_str()),
            )
            .await;
            session.push_flash(crate::session::FlashLevel::Success, t!("Role updated."));
            crate::form::saved_response(htmx, &format!("{}/roles", state.admin_path))
        }
        Err(_) => invalid_response(
            htmx,
            build(
                &state,
                &action,
                Some(t!("Could not save. The code may already be in use.")),
                &code,
                &name,
                &perms,
                shell,
            ),
        ),
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
fn group_permissions(
    registry: &[Permission],
    selected: &[String],
    shell: &Shell,
) -> Vec<PermGroupView> {
    let mut groups: Vec<PermGroupView> = Vec::new();
    for permission in registry {
        let check = PermCheckView {
            code: permission.code.clone(),
            label: shell.tt(&permission.label),
            checked: selected.iter().any(|s| s == &permission.code),
        };
        // Merge by the localized group heading (same source localizes identically).
        let gname = shell.tt(&permission.group);
        match groups.iter_mut().find(|g| g.name == gname) {
            Some(group) => group.perms.push(check),
            None => groups.push(PermGroupView {
                name: gname,
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
    error: Option<Text>,
    code: &str,
    name: &str,
    selected: &[String],
    shell: Shell,
) -> RolesFormTemplate {
    let groups = group_permissions(&state.permissions, selected, &shell);
    let title = shell.tt(&t!("Role"));
    let error = error.map(|e| shell.tt(&e));
    RolesFormTemplate {
        shell,
        title,
        action: action.to_string(),
        cancel_path: format!("{}/roles", state.admin_path),
        error,
        code: code.to_string(),
        name: name.to_string(),
        groups,
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

/// The form alone, for an HTMX submit that swaps it in place.
#[derive(Template)]
#[template(path = "_roles_form_fields.html")]
struct RolesFormFragment {
    shell: Shell,
    action: String,
    cancel_path: String,
    error: Option<String>,
    code: String,
    name: String,
    groups: Vec<PermGroupView>,
}

/// The 422 response for a rejected save: the form alone under HTMX, the whole
/// page otherwise, so the screen still works with scripting off.
fn invalid_response(htmx: bool, page: RolesFormTemplate) -> Response {
    let body = if htmx {
        render(RolesFormFragment {
            shell: page.shell,
            action: page.action,
            cancel_path: page.cancel_path,
            error: page.error,
            code: page.code,
            name: page.name,
            groups: page.groups,
        })
    } else {
        render(page)
    };
    (StatusCode::UNPROCESSABLE_ENTITY, body).into_response()
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
        let groups = group_permissions(&registry(), &["acme.publish".to_string()], &Shell::test());
        assert_eq!(groups[0].name, "Backend");
        assert_eq!(groups[1].name, "Content");
        assert!(!groups[0].perms[0].checked);
        assert!(groups[1].perms[0].checked);
    }
}

/// The roles form answers HTMX the same way the descriptor form does.
#[cfg(test)]
mod htmx_tests {
    use super::*;
    use laterite_core::testing::{connect_test, TestGuard};
    use laterite_core::Db;

    async fn test_db() -> (Db, TestGuard) {
        connect_test(&[laterite_auth::migrations()]).await
    }

    fn htmx_headers() -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert("hx-request", "true".parse().unwrap());
        h
    }

    /// Missing the required code, so the handler refuses before touching the db.
    fn invalid() -> Vec<(String, String)> {
        vec![("name".to_string(), "Editor".to_string())]
    }

    fn valid() -> Vec<(String, String)> {
        vec![
            ("code".to_string(), "editor".to_string()),
            ("name".to_string(), "Editor".to_string()),
        ]
    }

    async fn post(pairs: Vec<(String, String)>, headers: axum::http::HeaderMap) -> Response {
        let (db, _guard) = test_db().await;
        let state = AdminState::new(
            laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db,
        );
        create(
            State(state),
            Extension(Shell::test()),
            Extension(crate::session::SessionHandle::from_blob(None)),
            Extension(crate::audit::test_actor()),
            headers,
            Form(pairs),
        )
        .await
    }

    async fn body_of(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn an_htmx_submit_that_fails_returns_the_form_alone() {
        let resp = post(invalid(), htmx_headers()).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = body_of(resp).await;
        assert!(html.contains("<form"), "the form comes back");
        assert!(html.contains("lat-alert"), "carrying the error");
        assert!(html.contains("lat-permgroup"), "and the permission editor");
        assert!(!html.contains("<body"), "no page chrome");
    }

    #[tokio::test]
    async fn a_plain_submit_that_fails_returns_the_whole_page() {
        let resp = post(invalid(), axum::http::HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = body_of(resp).await;
        assert!(
            html.contains("<body"),
            "the screen still works without htmx"
        );
        assert!(html.contains("lat-alert"));
    }

    #[tokio::test]
    async fn an_htmx_submit_that_succeeds_redirects_by_header() {
        let resp = post(valid(), htmx_headers()).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(resp.headers().get("HX-Redirect").unwrap(), "/admin/roles");
    }

    #[tokio::test]
    async fn a_plain_submit_that_succeeds_redirects_normally() {
        let resp = post(valid(), axum::http::HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/admin/roles");
    }
}
