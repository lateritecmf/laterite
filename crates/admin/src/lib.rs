//! Laterite admin: the operator-facing web surface.
//!
//! An Axum router mounted at `/admin`: a login screen and session cookie
//! verified against `laterite-auth`, and descriptor-driven screens.
//!
//! Screens are **resources**: a module declares a [`Resource`] (a
//! [`list::ListConfig`], optionally a [`form::FormConfig`], a base path, and a
//! menu label), and the framework mounts the list, create, and edit routes and
//! adds it to the menu. This is the extension point that lets an application
//! contribute its own admin screens, the way October plugins register
//! controllers. The framework's own screens (users, roles) are just built-in
//! resources.

pub mod form;
pub mod list;
mod sql;

use std::collections::HashMap;
use std::sync::Arc;

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

/// Shared state for the admin router. Constructed by [`router`].
#[derive(Clone)]
pub(crate) struct AdminState {
    auth: AuthService,
    pool: PgPool,
    nav: Arc<Vec<NavLink>>,
}

impl AdminState {
    #[cfg(test)]
    pub(crate) fn new(auth: AuthService, pool: PgPool) -> Self {
        Self {
            auth,
            pool,
            nav: Arc::new(Vec::new()),
        }
    }
}

#[derive(Clone)]
struct NavLink {
    label: String,
    path: String,
}

/// An admin resource: a list screen, optionally with a create/edit form, mounted
/// under `base_path` and shown in the menu as `nav_label`.
pub struct Resource {
    pub base_path: String,
    pub nav_label: String,
    pub list: list::ListConfig,
    pub form: Option<form::FormConfig>,
}

/// Builds the admin router. `app_resources` are the application's own screens;
/// they are mounted alongside the framework's built-in resources.
pub fn router(auth: AuthService, pool: PgPool, app_resources: Vec<Resource>) -> Router {
    let mut resources = builtin_resources();
    resources.extend(app_resources);
    let nav = resources
        .iter()
        .map(|r| NavLink {
            label: r.nav_label.clone(),
            path: r.base_path.clone(),
        })
        .collect::<Vec<_>>();
    let state = AdminState {
        auth,
        pool,
        nav: Arc::new(nav),
    };

    let mut protected = Router::new().route("/admin", get(dashboard));
    for resource in &resources {
        protected = mount_resource(protected, resource);
    }
    protected = protected.route("/admin/logout", post(logout));

    protected
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        // Public routes (not covered by the guard above).
        .route("/admin/login", get(login_form).post(login_submit))
        .with_state(state)
}

/// Mounts a resource's list, create, and edit routes as generic handlers that
/// carry the resource's descriptors.
fn mount_resource(router: Router<AdminState>, resource: &Resource) -> Router<AdminState> {
    let base = resource.base_path.clone();
    let list_cfg = resource.list.clone();
    let mut router = router.route(
        &base,
        get(
            move |State(state): State<AdminState>, Query(params): Query<list::ListParams>| {
                let cfg = list_cfg.clone();
                async move { list::handle(&state, &cfg, params).await }
            },
        ),
    );

    if let Some(form_cfg) = resource.form.clone() {
        let (new_cfg, create_cfg) = (form_cfg.clone(), form_cfg.clone());
        router = router.route(
            &format!("{base}/new"),
            get(move || {
                let cfg = new_cfg.clone();
                async move { form::new_form(&cfg) }
            })
            .post(
                move |State(state): State<AdminState>,
                      Form(data): Form<HashMap<String, String>>| {
                    let cfg = create_cfg.clone();
                    async move { form::create(&state, &cfg, data).await }
                },
            ),
        );

        let (edit_cfg, update_cfg) = (form_cfg.clone(), form_cfg.clone());
        router = router.route(
            &format!("{base}/{{id}}/edit"),
            get(
                move |State(state): State<AdminState>, Path(id): Path<String>| {
                    let cfg = edit_cfg.clone();
                    async move { form::edit_form(&state, &cfg, id).await }
                },
            )
            .post(
                move |State(state): State<AdminState>,
                      Path(id): Path<String>,
                      Form(data): Form<HashMap<String, String>>| {
                    let cfg = update_cfg.clone();
                    async move { form::update(&state, &cfg, id, data).await }
                },
            ),
        );
    }
    router
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

async fn dashboard(
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    render(DashboardTemplate {
        full_name: user.user.full_name(),
        username: user.user.username,
        nav: state
            .nav
            .iter()
            .map(|n| NavView {
                label: n.label.clone(),
                path: n.path.clone(),
            })
            .collect(),
    })
}

/// The framework's own admin screens.
fn builtin_resources() -> Vec<Resource> {
    vec![
        Resource {
            base_path: "/admin/users".to_string(),
            nav_label: "Backend Users".to_string(),
            list: backend_users_list_config(),
            form: None,
        },
        Resource {
            base_path: "/admin/roles".to_string(),
            nav_label: "Roles".to_string(),
            list: roles_list_config(),
            form: Some(role_form_config()),
        },
    ]
}

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
    nav: Vec<NavView>,
}

struct NavView {
    label: String,
    path: String,
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
