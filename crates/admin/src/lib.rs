//! Laterite admin: the operator-facing web surface.
//!
//! An Axum router mounted at `/admin`: a login screen and session cookie
//! verified against `laterite-auth`, and descriptor-driven screens rendered by
//! generic handlers. Screens are data: a [`list::ListConfig`] or a
//! [`form::FormConfig`] describes an entity, and generic code renders and
//! persists it.

pub mod form;
pub mod list;
mod sql;

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use laterite_auth::{AuthService, AuthenticatedUser, RequestContext};
use serde::Deserialize;
use sqlx::PgPool;

const SESSION_COOKIE: &str = "laterite_session";

/// Shared state for the admin router.
#[derive(Clone)]
pub struct AdminState {
    pub auth: AuthService,
    pub pool: PgPool,
}

/// Builds the admin router. Routes live under `/admin`; the caller mounts it on
/// the application's root router.
pub fn router(state: AdminState) -> Router {
    Router::new()
        // Protected routes, then apply the auth guard only to those added so far.
        .route("/admin", get(dashboard))
        .route("/admin/users", get(users_list))
        .route("/admin/roles", get(roles_list))
        .route("/admin/roles/new", get(role_new).post(role_create))
        .route("/admin/roles/{id}/edit", get(role_edit).post(role_update))
        .route("/admin/logout", post(logout))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        // Public routes (not covered by the guard above).
        .route("/admin/login", get(login_form).post(login_submit))
        .with_state(state)
}

/// Redirects unauthenticated requests to the login screen, and injects the
/// resolved identity into request extensions for downstream handlers.
async fn require_auth(
    State(state): State<AdminState>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let identity = match jar.get(SESSION_COOKIE) {
        Some(cookie) => state.auth.verify_session(cookie.value()).await.ok(),
        None => None,
    };
    match identity {
        Some(user) => {
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        None => Redirect::to("/admin/login").into_response(),
    }
}

async fn login_form() -> Response {
    render(LoginTemplate { error: None })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login_submit(
    State(state): State<AdminState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Response {
    match state
        .auth
        .authenticate(&form.username, &form.password, &RequestContext::default())
        .await
    {
        Ok(session) => {
            let cookie = Cookie::build((SESSION_COOKIE, session.token))
                .path("/admin")
                .http_only(true)
                .same_site(SameSite::Lax)
                .build();
            (jar.add(cookie), Redirect::to("/admin")).into_response()
        }
        Err(_) => render(LoginTemplate {
            error: Some("Invalid username or password.".to_string()),
        }),
    }
}

async fn logout(State(state): State<AdminState>, jar: CookieJar) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let _ = state.auth.logout(cookie.value()).await;
    }
    let removal = Cookie::build((SESSION_COOKIE, "")).path("/admin").build();
    (jar.remove(removal), Redirect::to("/admin/login")).into_response()
}

async fn dashboard(Extension(user): Extension<AuthenticatedUser>) -> Response {
    render(DashboardTemplate {
        full_name: user.user.full_name(),
        username: user.user.username,
    })
}

async fn users_list(
    State(state): State<AdminState>,
    Query(params): Query<list::ListParams>,
) -> Response {
    list::handle(&state, &backend_users_list_config(), params).await
}

async fn roles_list(
    State(state): State<AdminState>,
    Query(params): Query<list::ListParams>,
) -> Response {
    list::handle(&state, &roles_list_config(), params).await
}

async fn role_new() -> Response {
    form::new_form(&role_form_config())
}

async fn role_create(
    State(state): State<AdminState>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    form::create(&state, &role_form_config(), data).await
}

async fn role_edit(State(state): State<AdminState>, Path(id): Path<String>) -> Response {
    form::edit_form(&state, &role_form_config(), id).await
}

async fn role_update(
    State(state): State<AdminState>,
    Path(id): Path<String>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    form::update(&state, &role_form_config(), id, data).await
}

/// The built-in list view for backend (operator) users.
fn backend_users_list_config() -> list::ListConfig {
    list::ListConfig {
        entity: "backend_users".to_string(),
        title: "Backend Users".to_string(),
        columns: vec![
            list::ListColumn::new("username", "Username"),
            list::ListColumn::new("email", "Email"),
            list::ListColumn::new("first_name", "First name"),
            list::ListColumn::new("last_name", "Last name"),
            list::ListColumn::new("is_superuser", "Superuser"),
            list::ListColumn::new("is_active", "Active"),
            list::ListColumn::new("created_at", "Created"),
        ],
        order_by: "created_at".to_string(),
        order_dir: list::SortDir::Desc,
        per_page: 25,
        id_field: "id".to_string(),
        edit_base: None,
    }
}

/// The built-in list view for backend roles.
fn roles_list_config() -> list::ListConfig {
    list::ListConfig {
        entity: "backend_roles".to_string(),
        title: "Roles".to_string(),
        columns: vec![
            list::ListColumn::new("code", "Code"),
            list::ListColumn::new("name", "Name"),
            list::ListColumn::new("created_at", "Created"),
        ],
        order_by: "created_at".to_string(),
        order_dir: list::SortDir::Desc,
        per_page: 25,
        id_field: "id".to_string(),
        edit_base: Some("/admin/roles".to_string()),
    }
}

/// The built-in create/edit form for backend roles.
fn role_form_config() -> form::FormConfig {
    form::FormConfig {
        entity: "backend_roles".to_string(),
        title: "Role".to_string(),
        base_path: "/admin/roles".to_string(),
        id_field: "id".to_string(),
        fields: vec![
            form::FormField::text("code", "Code").required(),
            form::FormField::text("name", "Name").required(),
        ],
    }
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    full_name: String,
    username: String,
}

/// Renders a template to an HTML response, mapping a render failure to a 500.
pub(crate) fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(_) => render_error(),
    }
}

pub(crate) fn render_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
}

pub(crate) fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not found").into_response()
}
