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
use laterite_auth::{AuthenticatedUser, NewOperator, PermissionSet};
use laterite_core::i18n::Text;
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
            let label = laterite_auth::store::find_user_by_id(&state.db, target)
                .await
                .ok()
                .flatten()
                .map(|u| u.username);
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
                label.as_deref(),
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

/// Gives an account a new temporary password on an administrator's behalf.
/// Never your own (that is Preferences), and never an account holding more
/// than you do: signing in as it would be an escalation.
pub(crate) async fn reset_password(
    State(state): State<AdminState>,
    Extension(editor): Extension<AuthenticatedUser>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    editor
        .require(MANAGE_USERS)
        .map_err(|_| AdminError::Forbidden)?;
    let target: i64 = id.parse().map_err(|_| AdminError::NotFound)?;
    let back = format!("{}/users/{id}/edit", state.admin_path);
    let user = laterite_auth::store::find_user_by_id(&state.db, target)
        .await?
        .ok_or(AdminError::NotFound)?;
    if target == editor.user.id {
        session.push_flash(
            crate::session::FlashLevel::Error,
            t!("Change your own password under Preferences."),
        );
        return Ok(Redirect::to(&back).into_response());
    }
    if !may_manage_credentials(&state, &editor, &user).await? {
        session.push_flash(
            crate::session::FlashLevel::Error,
            t!("You cannot reset the password of an account holding more than you do."),
        );
        return Ok(Redirect::to(&back).into_response());
    }
    let actor = laterite_core::Actor::user(editor.user.id, editor.user.username.clone());
    match state.auth.reset_operator_password(target, &actor).await {
        Ok(password) => session.push_flash_sticky(
            crate::session::FlashLevel::Success,
            t!(
                "Reset {username}. Their temporary password, shown only now: {password}. Every device was signed out.",
                username = user.username.clone(),
                password = password
            ),
        ),
        Err(e) => {
            tracing::error!(error = %e, "resetting a password failed");
            session.push_flash(
                crate::session::FlashLevel::Error,
                t!("The password could not be reset."),
            );
        }
    }
    Ok(Redirect::to(&back).into_response())
}

/// Clears a sign-in lockout so the account can sign in again now.
pub(crate) async fn unlock(
    State(state): State<AdminState>,
    Extension(editor): Extension<AuthenticatedUser>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Path(id): Path<String>,
) -> Result<Response, AdminError> {
    editor
        .require(MANAGE_USERS)
        .map_err(|_| AdminError::Forbidden)?;
    let target: i64 = id.parse().map_err(|_| AdminError::NotFound)?;
    let user = laterite_auth::store::find_user_by_id(&state.db, target)
        .await?
        .ok_or(AdminError::NotFound)?;
    state.auth.unlock(&user.username).await?;
    crate::audit::record(
        &state,
        &editor,
        "backend.user.unlock",
        Some("backend_user"),
        Some(&id),
        Some(&user.username),
        None,
    )
    .await;
    session.push_flash(
        crate::session::FlashLevel::Success,
        t!("That account can sign in again."),
    );
    Ok(Redirect::to(&format!("{}/users/{id}/edit", state.admin_path)).into_response())
}

/// Whether `editor` may take over `target`'s credentials: a superuser's only by
/// a superuser, and otherwise only an account whose every role grants nothing
/// the editor lacks, the rule role assignment applies.
async fn may_manage_credentials(
    state: &AdminState,
    editor: &AuthenticatedUser,
    target: &laterite_auth::BackendUser,
) -> Result<bool, AdminError> {
    if target.is_superuser {
        return Ok(editor.user.is_superuser);
    }
    let held = laterite_auth::store::user_role_ids(&state.db, target.id).await?;
    let roles = laterite_auth::store::list_roles(&state.db).await?;
    Ok(roles
        .iter()
        .filter(|r| held.contains(&r.id))
        .all(|r| r.permissions.iter().all(|p| editor.permissions.allows(p))))
}

#[derive(serde::Deserialize)]
pub(crate) struct ActiveForm {
    /// "1" to activate, anything else to deactivate.
    active: String,
}

/// Renders the form for a new operator.
pub(crate) async fn new_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(editor): Extension<AuthenticatedUser>,
) -> Result<Response, AdminError> {
    let roles = role_rows(
        laterite_auth::store::list_roles(&state.db).await?,
        &[],
        &editor.permissions,
        false,
    );
    Ok(render(new_page(
        &state,
        shell,
        None,
        NewFields::default(),
        roles,
    )))
}

/// Creates an operator with a generated temporary password, shown once, then
/// opens their edit screen. The password is never typed by the administrator,
/// so nobody but its holder ever knows a password that stays in use.
pub(crate) async fn create(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(session): Extension<crate::session::SessionHandle>,
    Extension(editor): Extension<AuthenticatedUser>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AdminError> {
    editor
        .require(MANAGE_USERS)
        .map_err(|_| AdminError::Forbidden)?;
    let (fields, wanted) = NewFields::parse(&pairs);
    let roles = role_rows(
        laterite_auth::store::list_roles(&state.db).await?,
        &wanted,
        &editor.permissions,
        false,
    );
    if fields.username.is_empty() || fields.email.is_empty() || fields.first_name.is_empty() {
        return Ok(render(new_page(
            &state,
            shell,
            Some(t!("Username, email and first name are required.")),
            fields,
            roles,
        )));
    }
    if !fields.email.contains('@') {
        return Ok(render(new_page(
            &state,
            shell,
            Some(t!("That is not an email address.")),
            fields,
            roles,
        )));
    }
    let password = laterite_auth::password::generate();
    let actor = laterite_core::Actor::user(editor.user.id, editor.user.username.clone());
    let created = state
        .auth
        .create_operator(
            NewOperator {
                username: &fields.username,
                email: &fields.email,
                first_name: &fields.first_name,
                last_name: (!fields.last_name.is_empty()).then_some(fields.last_name.as_str()),
                password: &password,
                timezone: None,
            },
            &actor,
        )
        .await;
    let id = match created {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "creating an operator failed");
            return Ok(render(new_page(
                &state,
                shell,
                Some(t!(
                    "Could not create the account. The username or email may already be taken."
                )),
                fields,
                roles,
            )));
        }
    };
    // Only roles the editor may grant, the same rule the edit screen applies.
    let granted: Vec<i64> = roles
        .iter()
        .filter(|r| r.held && r.changeable)
        .map(|r| r.id)
        .collect();
    laterite_auth::store::set_user_roles(&state.db, id, &granted).await?;
    state.auth.require_password_change(id).await?;
    session.push_flash_sticky(
        crate::session::FlashLevel::Success,
        t!(
            "Created {username}. Their temporary password, shown only now: {password}",
            username = fields.username.clone(),
            password = password
        ),
    );
    Ok(Redirect::to(&format!("{}/users/{id}/edit", state.admin_path)).into_response())
}

/// What the new-operator form carries, kept across a refused save.
#[derive(Default)]
struct NewFields {
    username: String,
    email: String,
    first_name: String,
    last_name: String,
}

impl NewFields {
    /// The fields and the role ids ticked.
    fn parse(pairs: &[(String, String)]) -> (Self, Vec<i64>) {
        let mut fields = Self::default();
        let mut roles = Vec::new();
        for (key, value) in pairs {
            match key.as_str() {
                "username" => fields.username = value.trim().to_string(),
                "email" => fields.email = value.trim().to_string(),
                "first_name" => fields.first_name = value.trim().to_string(),
                "last_name" => fields.last_name = value.trim().to_string(),
                "role" => {
                    if let Ok(id) = value.parse() {
                        roles.push(id);
                    }
                }
                _ => {}
            }
        }
        (fields, roles)
    }
}

fn new_page(
    state: &AdminState,
    shell: Shell,
    error: Option<Text>,
    fields: NewFields,
    roles: Vec<RoleRowView>,
) -> UsersNewTemplate {
    let error = error.map(|e| shell.tt(&e));
    UsersNewTemplate {
        shell,
        action: format!("{}/users/new", state.admin_path),
        cancel_path: format!("{}/users", state.admin_path),
        error,
        username: fields.username,
        email: fields.email,
        first_name: fields.first_name,
        last_name: fields.last_name,
        roles,
    }
}

#[derive(Template)]
#[template(path = "users_new.html")]
struct UsersNewTemplate {
    shell: Shell,
    action: String,
    cancel_path: String,
    error: Option<String>,
    username: String,
    email: String,
    first_name: String,
    last_name: String,
    /// Every role, ticked as submitted; one the editor may not grant is locked.
    roles: Vec<RoleRowView>,
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
                Alias::new("password_changed_at"),
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
    let password_changed = row
        .get_text_opt("password_changed_at")
        .ok()
        .flatten()
        .map(|raw| {
            let locale = crate::list::date_locale(shell.locale());
            crate::list::format_ts(&raw, shell.tz, locale, "%-d %b %Y, %H:%M")
        });
    // Refused server-side either way; hiding the control keeps the screen from
    // offering an action it will not carry out.
    let can_change_state =
        editor.user.id.to_string() != id && editor.permissions.allows(crate::users::MANAGE_USERS);
    let perms_json = row.get_text("permissions").unwrap_or_default();
    let overrides: HashMap<String, i64> = serde_json::from_str(&perms_json).unwrap_or_default();
    // A superuser holds everything already, so roles would change nothing.
    let roles = if is_superuser {
        Vec::new()
    } else {
        let target_id = id.parse::<i64>().unwrap_or_default();
        role_rows(
            laterite_auth::store::list_roles(&state.db).await?,
            &laterite_auth::store::user_role_ids(&state.db, target_id).await?,
            &editor.permissions,
            editor.user.id.to_string() == id,
        )
    };
    let target_id = id.parse::<i64>().unwrap_or_default();
    let target = laterite_auth::store::find_user_by_id(&state.db, target_id).await?;
    let can_reset = match &target {
        Some(user) => {
            editor.user.id != user.id
                && editor.permissions.allows(crate::users::MANAGE_USERS)
                && may_manage_credentials(&state, &editor, user).await?
        }
        None => false,
    };
    let locked = state.auth.is_locked_out(&username).await.unwrap_or(false);
    let mut page = build(
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
        roles,
    );
    page.password_changed = password_changed;
    page.can_reset = can_reset;
    page.reset_action = format!("{}/users/{id}/password", state.admin_path);
    page.locked = locked;
    page.unlock_action = format!("{}/users/{id}/unlock", state.admin_path);
    Ok(render(page))
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
            .columns([
                Alias::new("username"),
                Alias::new("is_superuser"),
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
    // A superuser has no editable overrides; nothing to save.
    if row.get_bool("is_superuser").unwrap_or(false) {
        return Ok(Redirect::to(&format!("{}/users/{id}/edit", state.admin_path)).into_response());
    }
    let target_id = id.parse::<i64>().map_err(|_| AdminError::NotFound)?;

    let perms_json = row.get_text("permissions").unwrap_or_default();
    let mut overrides: HashMap<String, i64> = serde_json::from_str(&perms_json).unwrap_or_default();
    let submitted = parse_states(&pairs);

    // Roles: keep every one the editor may not change exactly as it was, and
    // take the submitted set for the rest. A crafted id the screen would have
    // locked therefore cannot be added or removed.
    let held = laterite_auth::store::user_role_ids(&state.db, target_id).await?;
    let ticked: Vec<i64> = pairs
        .iter()
        .filter(|(k, _)| k == "role")
        .filter_map(|(_, v)| v.trim().parse::<i64>().ok())
        .collect();
    let rows = role_rows(
        laterite_auth::store::list_roles(&state.db).await?,
        &held,
        &editor.permissions,
        editor.user.id == target_id,
    );
    let next: Vec<i64> = rows
        .iter()
        .filter(|r| {
            if r.changeable {
                ticked.contains(&r.id)
            } else {
                r.held
            }
        })
        .map(|r| r.id)
        .collect();
    if next != held {
        laterite_auth::store::set_user_roles(&state.db, target_id, &next).await?;
    }

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
        Some(row.get_text("username").unwrap_or_default().as_str()),
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
    roles: Vec<RoleRowView>,
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
        roles_locked: !roles.is_empty() && roles.iter().all(|r| !r.changeable),
        roles,
        password_changed: None,
        can_reset: false,
        reset_action: String::new(),
        locked: false,
        unlock_action: String::new(),
    }
}

/// The roles to show, and whether this operator may change each.
///
/// A role grants every permission it names, so offering one the editor does not
/// hold themselves would let them escalate through it: the same rule the
/// override rows follow. Their own roles are locked too, so nobody signs
/// themselves out of the panel.
fn role_rows(
    all: Vec<laterite_auth::store::RoleSummary>,
    held: &[i64],
    editor: &PermissionSet,
    own_account: bool,
) -> Vec<RoleRowView> {
    all.into_iter()
        .map(|role| RoleRowView {
            held: held.contains(&role.id),
            changeable: !own_account && role.permissions.iter().all(|p| editor.allows(p)),
            id: role.id,
            name: role.name,
            code: role.code,
        })
        .collect()
}

/// One role on the assignment list.
struct RoleRowView {
    id: i64,
    name: String,
    code: String,
    held: bool,
    /// Whether this operator may change it: they hold everything the role
    /// grants, and it is not their own account.
    changeable: bool,
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
    /// Every role, with the ones this operator holds ticked.
    roles: Vec<RoleRowView>,
    /// Whether any role is changeable, so the screen can say why not.
    roles_locked: bool,
    /// When the password last changed, formatted; `None` when not recorded.
    password_changed: Option<String>,
    /// Whether the viewing operator may give this account a temporary
    /// password: not their own, and holding nothing they lack.
    can_reset: bool,
    reset_action: String,
    /// Failed sign-ins have locked the account out for now.
    locked: bool,
    unlock_action: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Vec<Permission> {
        vec![
            Permission::new("backend.manage_users", "Manage backend users", "Backend"),
            Permission::new("acme.publish", "Publish", "Content"),
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
