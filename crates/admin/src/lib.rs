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
pub mod settings;
mod sql;

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono_tz::{Tz, TZ_VARIANTS};
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
    settings: Arc<Vec<settings::SettingsItem>>,
    secure_cookie: bool,
    timezone: Tz,
}

impl AdminState {
    #[cfg(test)]
    pub(crate) fn new(auth: AuthService, pool: PgPool) -> Self {
        Self {
            auth,
            pool,
            nav: Arc::new(Vec::new()),
            settings: Arc::new(Vec::new()),
            secure_cookie: false,
            timezone: Tz::UTC,
        }
    }
}

/// Deployment-level admin settings passed to [`router`]. Per-install brand and
/// per-operator preferences are settings/preferences, not deployment config.
#[derive(Clone)]
pub struct AdminConfig {
    /// Set the `Secure` attribute on the session cookie. Enable behind HTTPS in
    /// production; leave off for plain-HTTP local development.
    pub secure_cookie: bool,
    /// Default display timezone for the admin (an IANA name like `Asia/Kolkata`).
    /// Storage is UTC; this only affects rendering. Invalid or empty falls back
    /// to UTC. An operator's own preference overrides it (later).
    pub timezone: String,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            secure_cookie: false,
            timezone: "UTC".to_string(),
        }
    }
}

#[derive(Clone)]
struct NavLink {
    label: String,
    path: String,
}

/// The chrome shared by every authenticated page: the top-nav links and the
/// signed-in operator. Built once by the auth guard and injected into request
/// extensions, so page handlers render inside the same shell without each
/// rebuilding it. Templates embed it as `shell` and `base.html` renders it.
#[derive(Clone)]
pub(crate) struct Shell {
    nav: Vec<NavView>,
    full_name: String,
    initial: String,
    /// The timezone this operator's timestamps render in, resolved once per
    /// request: the operator's own preference if set and valid, else the
    /// deployment default. List and detail screens format dates in it.
    tz: Tz,
}

impl Shell {
    fn new(nav: &[NavLink], user: &AuthenticatedUser, default_tz: Tz) -> Self {
        let full_name = user.user.full_name();
        let initial = full_name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        Shell {
            nav: nav
                .iter()
                .map(|n| NavView {
                    label: n.label.clone(),
                    path: n.path.clone(),
                })
                .collect(),
            full_name,
            initial,
            tz: resolve_display_tz(user.user.timezone.as_deref(), default_tz),
        }
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Shell {
            nav: Vec::new(),
            full_name: "Test Operator".to_string(),
            initial: "T".to_string(),
            tz: Tz::UTC,
        }
    }
}

/// Resolves the timezone an operator's timestamps render in: their own
/// preference when it is set and a valid IANA name, otherwise the deployment
/// default. An unparseable stored value falls back rather than erroring.
fn resolve_display_tz(preference: Option<&str>, default_tz: Tz) -> Tz {
    preference
        .and_then(|name| name.parse::<Tz>().ok())
        .unwrap_or(default_tz)
}

/// An admin resource: a list screen, optionally with a create/edit form, mounted
/// under `base_path` and shown in the menu as `nav_label`.
pub struct Resource {
    pub base_path: String,
    pub nav_label: String,
    pub list: list::ListConfig,
    pub form: Option<form::FormConfig>,
}

/// The migration sets for every module the admin mounts: the auth schema
/// (users, roles, sessions, access log) and the settings store. Run these
/// before serving [`router`] so its built-in screens have their tables, so an
/// application never has to know which framework modules the admin pulls in.
///
/// An application with its own modules appends their sets:
///
/// ```no_run
/// # async fn f(pool: sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
/// let migrations = laterite_admin::builtin_migrations();
/// // migrations.extend([my_module::migrations()]);
/// laterite_core::migrate::run(&pool, &migrations).await?;
/// # Ok(()) }
/// ```
pub fn builtin_migrations() -> Vec<laterite_core::ModuleMigrations> {
    vec![laterite_auth::migrations(), laterite_settings::migrations()]
}

/// Builds the admin router. `app_resources` are the application's own list/form
/// screens; `app_settings` are its settings models. Both are mounted alongside
/// the framework's built-in resources.
pub fn router(
    auth: AuthService,
    pool: PgPool,
    app_resources: Vec<Resource>,
    app_settings: Vec<settings::SettingsItem>,
    config: AdminConfig,
) -> Router {
    let mut resources = builtin_resources();
    let mut settings = builtin_settings();
    settings.extend(app_settings);

    // Main menu (top nav): Dashboard, the application's own sections, then
    // Settings. Built-in Users and Roles are settings items (see the settings
    // menu), not main-menu tabs.
    let mut nav = vec![NavLink {
        label: "Dashboard".to_string(),
        path: "/admin".to_string(),
    }];
    for resource in &app_resources {
        nav.push(NavLink {
            label: resource.nav_label.clone(),
            path: resource.base_path.clone(),
        });
    }
    nav.push(NavLink {
        label: "Settings".to_string(),
        path: "/admin/settings".to_string(),
    });
    resources.extend(app_resources);

    let state = AdminState {
        auth,
        pool,
        nav: Arc::new(nav),
        settings: Arc::new(settings),
        secure_cookie: config.secure_cookie,
        timezone: config.timezone.parse().unwrap_or(Tz::UTC),
    };

    let mut protected = Router::new().route("/admin", get(dashboard));
    for resource in &resources {
        protected = mount_resource(protected, resource);
    }
    protected = protected
        .route("/admin/settings", get(settings_index))
        .route(
            "/admin/settings/{code}",
            get(settings_edit).post(settings_update),
        )
        .route(
            "/admin/preferences",
            get(preferences_form).post(preferences_update),
        )
        .route("/admin/logout", post(logout));

    protected
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        // Public routes (not covered by the guard above): the login screen and
        // the embedded stylesheet and fonts (needed before authentication).
        .route("/admin/login", get(login_form).post(login_submit))
        .route("/admin/assets/laterite.css", get(asset_css))
        .route("/admin/assets/mark.svg", get(asset_mark))
        .route("/admin/assets/mark.png", get(asset_mark_png))
        .route("/admin/assets/fonts/{file}", get(asset_font))
        .with_state(state)
}

/// Serves the embedded brick mark (SVG badge, used for the favicon).
async fn asset_mark() -> Response {
    (
        [(header::CONTENT_TYPE, "image/svg+xml")],
        include_str!("../assets/mark.svg"),
    )
        .into_response()
}

/// Serves the embedded brick mark (PNG, the visible logo).
async fn asset_mark_png() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        &include_bytes!("../assets/mark.png")[..],
    )
        .into_response()
}

/// Serves the embedded admin stylesheet.
async fn asset_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../assets/laterite.css"),
    )
        .into_response()
}

/// Serves an embedded webfont by file name.
async fn asset_font(Path(file): Path<String>) -> Response {
    let bytes: &[u8] = match file.as_str() {
        "space-grotesk-500.woff2" => &include_bytes!("../assets/fonts/space-grotesk-500.woff2")[..],
        "space-grotesk-600.woff2" => &include_bytes!("../assets/fonts/space-grotesk-600.woff2")[..],
        "space-grotesk-700.woff2" => &include_bytes!("../assets/fonts/space-grotesk-700.woff2")[..],
        "ibm-plex-sans-400.woff2" => &include_bytes!("../assets/fonts/ibm-plex-sans-400.woff2")[..],
        "ibm-plex-sans-600.woff2" => &include_bytes!("../assets/fonts/ibm-plex-sans-600.woff2")[..],
        "ibm-plex-mono-400.woff2" => &include_bytes!("../assets/fonts/ibm-plex-mono-400.woff2")[..],
        "ibm-plex-mono-600.woff2" => &include_bytes!("../assets/fonts/ibm-plex-mono-600.woff2")[..],
        _ => return not_found(),
    };
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response()
}

/// Mounts a resource's list, create, and edit routes as generic handlers that
/// carry the resource's descriptors.
fn mount_resource(router: Router<AdminState>, resource: &Resource) -> Router<AdminState> {
    let base = resource.base_path.clone();
    let list_cfg = resource.list.clone();
    let mut router = router.route(
        &base,
        get(
            move |State(state): State<AdminState>,
                  Extension(shell): Extension<Shell>,
                  Query(params): Query<list::ListParams>| {
                let cfg = list_cfg.clone();
                async move { list::handle(&state, &cfg, params, shell).await }
            },
        ),
    );

    if let Some(form_cfg) = resource.form.clone() {
        let (new_cfg, create_cfg) = (form_cfg.clone(), form_cfg.clone());
        router = router.route(
            &format!("{base}/new"),
            get(move |Extension(shell): Extension<Shell>| {
                let cfg = new_cfg.clone();
                async move { form::new_form(&cfg, shell) }
            })
            .post(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Form(data): Form<HashMap<String, String>>| {
                    let cfg = create_cfg.clone();
                    async move { form::create(&state, &cfg, data, shell).await }
                },
            ),
        );

        let (edit_cfg, update_cfg) = (form_cfg.clone(), form_cfg.clone());
        router = router.route(
            &format!("{base}/{{id}}/edit"),
            get(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Path(id): Path<String>| {
                    let cfg = edit_cfg.clone();
                    async move { form::edit_form(&state, &cfg, id, shell).await }
                },
            )
            .post(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Path(id): Path<String>,
                      Form(data): Form<HashMap<String, String>>| {
                    let cfg = update_cfg.clone();
                    async move { form::update(&state, &cfg, id, data, shell).await }
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
            let shell = Shell::new(&state.nav, &user, state.timezone);
            request.extensions_mut().insert(user);
            request.extensions_mut().insert(shell);
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
                .secure(state.secure_cookie)
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
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    render(DashboardTemplate {
        username: user.user.username,
        shell,
    })
}

async fn settings_index(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
) -> Response {
    settings::index(&state.settings, shell)
}

async fn settings_edit(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path(code): Path<String>,
) -> Response {
    match state
        .settings
        .iter()
        .find(|item| item.code == code && item.link.is_none())
    {
        Some(item) => settings::edit_form(&state, item, shell).await,
        None => not_found(),
    }
}

async fn settings_update(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path(code): Path<String>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    match state
        .settings
        .iter()
        .find(|item| item.code == code && item.link.is_none())
    {
        Some(item) => settings::update(&state, item, data, shell).await,
        None => not_found(),
    }
}

#[derive(Deserialize)]
struct PreferencesQuery {
    saved: Option<String>,
}

/// The self-service Preferences screen for the signed-in operator.
async fn preferences_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<PreferencesQuery>,
) -> Response {
    render(preferences_view(
        &shell,
        &user,
        state.timezone,
        query.saved.is_some(),
        None,
    ))
}

#[derive(Deserialize)]
struct PreferencesForm {
    timezone: String,
}

async fn preferences_update(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Form(form): Form<PreferencesForm>,
) -> Response {
    let trimmed = form.timezone.trim();
    // An empty choice clears the preference so the operator inherits the default.
    let stored = if trimmed.is_empty() {
        None
    } else if trimmed.parse::<Tz>().is_ok() {
        Some(trimmed)
    } else {
        return render(preferences_view(
            &shell,
            &user,
            state.timezone,
            false,
            Some("That is not a recognised timezone."),
        ));
    };
    match state.auth.set_user_timezone(user.user.id, stored).await {
        Ok(()) => Redirect::to("/admin/preferences?saved=1").into_response(),
        Err(_) => render_error(),
    }
}

/// Builds the Preferences view. `shell.tz` is the timezone currently in force
/// (the operator's preference or the default); the operator's stored preference
/// selects the matching option, or the inherit option when unset.
fn preferences_view(
    shell: &Shell,
    user: &AuthenticatedUser,
    default_tz: Tz,
    saved: bool,
    error: Option<&str>,
) -> PreferencesTemplate {
    let current = user.user.timezone.as_deref();
    let zones = TZ_VARIANTS
        .iter()
        .map(|tz| TzOption {
            name: tz.name().to_string(),
            selected: current == Some(tz.name()),
        })
        .collect();
    PreferencesTemplate {
        shell: shell.clone(),
        zones,
        effective_tz: shell.tz.name().to_string(),
        default_tz: default_tz.name().to_string(),
        inherits: current.is_none(),
        saved,
        error: error.map(|e| e.to_string()),
    }
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

/// The framework's own settings items. The built-in Users and Roles resources
/// appear in the settings menu under a Users category (linking to their list
/// screens), rather than as main-menu tabs.
fn builtin_settings() -> Vec<settings::SettingsItem> {
    vec![
        settings::SettingsItem {
            code: "backend.administrators".to_string(),
            label: "Administrators".to_string(),
            description: "Manage backend administrator accounts.".to_string(),
            category: "Users".to_string(),
            order: 10,
            permission: None,
            link: Some("/admin/users".to_string()),
            fields: Vec::new(),
        },
        settings::SettingsItem {
            code: "backend.roles".to_string(),
            label: "Roles".to_string(),
            description: "Manage roles and their permissions.".to_string(),
            category: "Users".to_string(),
            order: 20,
            permission: None,
            link: Some("/admin/roles".to_string()),
            fields: Vec::new(),
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
            list::ListColumn::new("is_superuser", "Superuser").yes_no(),
            list::ListColumn::new("is_active", "Active").yes_no(),
            list::ListColumn::new("created_at", "Created").datetime(),
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
            list::ListColumn::new("created_at", "Created").datetime(),
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
    shell: Shell,
    username: String,
}

#[derive(Template)]
#[template(path = "preferences.html")]
struct PreferencesTemplate {
    shell: Shell,
    zones: Vec<TzOption>,
    /// The timezone dates currently render in for this operator.
    effective_tz: String,
    /// The deployment default, named in the inherit option.
    default_tz: String,
    /// Whether the operator currently inherits the default (no preference set).
    inherits: bool,
    saved: bool,
    error: Option<String>,
}

struct TzOption {
    name: String,
    selected: bool,
}

#[derive(Clone)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_tz_prefers_a_valid_operator_preference() {
        assert_eq!(
            resolve_display_tz(Some("Asia/Kolkata"), Tz::UTC),
            Tz::Asia__Kolkata
        );
    }

    #[test]
    fn display_tz_falls_back_when_unset_or_invalid() {
        let default = Tz::Europe__London;
        // No preference: use the deployment default.
        assert_eq!(resolve_display_tz(None, default), default);
        // Junk stored value: fall back rather than error.
        assert_eq!(resolve_display_tz(Some("Not/AZone"), default), default);
        assert_eq!(resolve_display_tz(Some(""), default), default);
    }

    #[sqlx::test(migrations = false)]
    async fn builtin_migrations_create_the_admin_tables(pool: PgPool) {
        laterite_core::migrate::run(&pool, &builtin_migrations())
            .await
            .unwrap();
        // A table from each bundled module exists, so an app that only ran
        // builtin_migrations has everything the admin's screens need.
        for table in ["backend_users", "settings"] {
            let exists: bool = sqlx::query_scalar(
                "select exists (select from information_schema.tables \
                 where table_schema = 'public' and table_name = $1)",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(exists, "{table} should exist after builtin_migrations");
        }
    }
}
