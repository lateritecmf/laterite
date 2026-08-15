//! Laterite admin: the operator-facing web surface.
//!
//! An Axum router mounted at `/admin`: a login screen and session cookie
//! verified against `laterite-auth`, and descriptor-driven screens.
//!
//! Screens are **resources**: a module declares a [`Resource`] (a
//! [`list::ListConfig`], optionally a [`form::FormConfig`], a base path, and a
//! menu label), and the framework mounts the list, create, and edit routes and
//! adds it to the menu. This is the extension point that lets an application
//! contribute its own admin screens. The framework's own screens (users, roles)
//! are just built-in resources.

pub mod form;
mod icons;
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
use laterite_auth::{AuthService, AuthenticatedUser, NewOperator, PermissionSet, RequestContext};
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
    /// An icon name (a Lucide subset, see [`icons`]), or `None` for a text-only
    /// tab. The built-in Dashboard and Settings entries set one.
    icon: Option<&'static str>,
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
    /// The context sidebar for the current section, resolved once per request
    /// from the path (see [`resolve_nav_context`]). Empty means no sidebar.
    /// `base.html` renders it, so any screen in a settings context shows it.
    sidebar: Vec<settings::CategoryView>,
}

impl Shell {
    fn new(
        nav: &[NavLink],
        user: &AuthenticatedUser,
        default_tz: Tz,
        sidebar: Vec<settings::CategoryView>,
        active_nav: Option<&str>,
    ) -> Self {
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
                    active: active_nav == Some(n.path.as_str()),
                    icon: n.icon.map(|name| icons::svg(Some(name))).unwrap_or(""),
                })
                .collect(),
            full_name,
            initial,
            tz: resolve_display_tz(user.user.timezone.as_deref(), default_tz),
            sidebar,
        }
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Shell {
            nav: Vec::new(),
            full_name: "Test Operator".to_string(),
            initial: "T".to_string(),
            tz: Tz::UTC,
            sidebar: Vec::new(),
        }
    }
}

/// Whether `path` sits in the settings context, and if so which item code is
/// active. A screen is in the settings context when it is the settings index or
/// a settings form, or when its path falls under a settings item's `link` (its
/// list, forms and sub-pages). The matching item is returned so the sidebar can
/// highlight it. `visible` is the operator's permitted items, so a linked
/// resource they cannot see never claims the context.
fn settings_context(visible: &[settings::SettingsItem], path: &str) -> (bool, Option<String>) {
    if path == "/admin/settings" {
        (true, None)
    } else if let Some(code) = path.strip_prefix("/admin/settings/") {
        (true, Some(code.to_string()))
    } else {
        // A linked resource: the settings item whose link is the longest prefix
        // of this path owns the context (so /admin/roles/5/edit still resolves).
        let active = visible
            .iter()
            .filter_map(|i| i.link.as_deref().map(|link| (link, &i.code)))
            .filter(|(link, _)| path == *link || path.starts_with(&format!("{link}/")))
            .max_by_key(|(link, _)| link.len())
            .map(|(_, code)| code.clone());
        (active.is_some(), active)
    }
}

/// The top-nav item to highlight for `path`. A screen in the settings context
/// lights the Settings tab (so a linked resource such as the users list keeps
/// Settings active); otherwise a section owns its own path subtree and stays
/// active across its sub-pages, with the longest matching prefix winning. The
/// `/admin` root is every path's ancestor, so it lights the Dashboard tab only
/// on an exact match: a screen belonging to no section (Preferences, say) lights
/// nothing rather than falling back to Dashboard.
fn active_nav_path(nav: &[NavLink], in_settings_context: bool, path: &str) -> Option<String> {
    if in_settings_context {
        return Some("/admin/settings".to_string());
    }
    nav.iter()
        .filter(|n| {
            path == n.path || (n.path != "/admin" && path.starts_with(&format!("{}/", n.path)))
        })
        .max_by_key(|n| n.path.len())
        .map(|n| n.path.clone())
}

/// Resolves the per-request navigation context from the descriptors: the
/// settings sidebar (empty when the screen sits outside any settings context)
/// and the top-nav item to highlight. The auth guard runs this once per request
/// and hands both to the [`Shell`].
fn resolve_nav_context(
    nav: &[NavLink],
    items: &[settings::SettingsItem],
    perms: &PermissionSet,
    path: &str,
) -> (Vec<settings::CategoryView>, Option<String>) {
    let visible = visible_settings(items, perms);
    let (in_context, active) = settings_context(&visible, path);
    let sidebar = if in_context {
        settings::sidebar_groups(&visible, active.as_deref())
    } else {
        Vec::new()
    };
    let active_nav = active_nav_path(nav, in_context, path);
    (sidebar, active_nav)
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
    /// The permission an operator must hold to reach any of the resource's
    /// routes. `None` leaves the resource open to any signed-in operator; a
    /// dotted string gates every route the resource mounts, and an operator who
    /// lacks it receives `403 Forbidden`. Superusers pass regardless.
    pub permission: Option<String>,
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
        icon: Some("layout-dashboard"),
    }];
    for resource in &app_resources {
        nav.push(NavLink {
            label: resource.nav_label.clone(),
            path: resource.base_path.clone(),
            icon: None,
        });
    }
    nav.push(NavLink {
        label: "Settings".to_string(),
        path: "/admin/settings".to_string(),
        icon: Some("settings"),
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
        protected = protected.merge(mount_resource(resource));
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
        // Public routes (not covered by the guard above): the login and first-run
        // setup screens and the embedded stylesheet and fonts (needed before
        // authentication).
        .route("/admin/login", get(login_form).post(login_submit))
        .route("/admin/setup", get(setup_form).post(setup_submit))
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

/// Builds a resource's list, create, and edit routes as generic handlers that
/// carry the resource's descriptors. When the resource sets a `permission`, every
/// route it mounts is wrapped in a guard that answers `403 Forbidden` for an
/// operator who lacks it; the caller merges the result into the protected router.
fn mount_resource(resource: &Resource) -> Router<AdminState> {
    let base = resource.base_path.clone();
    let list_cfg = resource.list.clone();
    let mut router = Router::new().route(
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

    // Gate every route the resource mounts on its permission. The auth guard
    // runs first and injects the identity, so the guard reads it from the
    // request extensions and answers 403 for an operator who lacks the grant.
    if let Some(permission) = resource.permission.clone() {
        let needed: Arc<str> = Arc::from(permission);
        router = router.route_layer(middleware::from_fn(
            move |Extension(user): Extension<AuthenticatedUser>, req: Request, next: Next| {
                let needed = needed.clone();
                async move {
                    if user.allows(&needed) {
                        next.run(req).await
                    } else {
                        forbidden()
                    }
                }
            },
        ));
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
            let path = request.uri().path().to_string();
            let (sidebar, active_nav) =
                resolve_nav_context(&state.nav, &state.settings, &user.permissions, &path);
            let shell = Shell::new(
                &state.nav,
                &user,
                state.timezone,
                sidebar,
                active_nav.as_deref(),
            );
            request.extensions_mut().insert(user);
            request.extensions_mut().insert(shell);
            next.run(request).await
        }
        None => Redirect::to("/admin/login").into_response(),
    }
}

async fn login_form(State(state): State<AdminState>) -> Response {
    // A fresh install with no operators goes to first-run setup instead.
    match state.auth.has_any_operator().await {
        Ok(false) => Redirect::to("/admin/setup").into_response(),
        Ok(true) => render(LoginTemplate { error: None }),
        Err(_) => render_error(),
    }
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
            let cookie = session_cookie(session.token, state.secure_cookie);
            (jar.add(cookie), Redirect::to("/admin")).into_response()
        }
        Err(_) => render(LoginTemplate {
            error: Some("Invalid username or password.".to_string()),
        }),
    }
}

/// Builds the session cookie, scoped to the admin and flagged `Secure` behind
/// HTTPS. Shared by login and first-run setup.
fn session_cookie(token: String, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path("/admin")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

#[derive(Deserialize)]
struct SetupForm {
    username: String,
    first_name: String,
    last_name: String,
    email: String,
    password: String,
    timezone: String,
}

/// The first-run setup screen: shown only while no operator exists, so a fresh
/// install can create its first administrator without the CLI.
async fn setup_form(State(state): State<AdminState>) -> Response {
    match state.auth.has_any_operator().await {
        Ok(true) => Redirect::to("/admin/login").into_response(),
        Ok(false) => render(setup_view(state.timezone, None)),
        Err(_) => render_error(),
    }
}

async fn setup_submit(
    State(state): State<AdminState>,
    jar: CookieJar,
    Form(form): Form<SetupForm>,
) -> Response {
    // Setup only ever creates the first operator; once one exists it is closed.
    match state.auth.has_any_operator().await {
        Ok(true) => return Redirect::to("/admin/login").into_response(),
        Ok(false) => {}
        Err(_) => return render_error(),
    }

    let username = form.username.trim();
    let email = form.email.trim();
    let first_name = form.first_name.trim();
    let last_name = form.last_name.trim();
    let tz = form.timezone.trim();
    if username.is_empty() || email.is_empty() || first_name.is_empty() || form.password.is_empty()
    {
        return render(setup_view(
            state.timezone,
            Some("Username, first name, email, and password are all required."),
        ));
    }
    // The setup select always carries a value, but guard against a bad one.
    if tz.parse::<Tz>().is_err() {
        return render(setup_view(
            state.timezone,
            Some("That is not a recognised timezone."),
        ));
    }

    let new = NewOperator {
        username,
        email,
        first_name,
        last_name: (!last_name.is_empty()).then_some(last_name),
        password: &form.password,
        timezone: Some(tz),
    };
    if state.auth.create_superuser(new).await.is_err() {
        return render(setup_view(
            state.timezone,
            Some("Could not create the account. The username or email may already be taken."),
        ));
    }

    // Sign the new administrator straight in through the normal login path.
    match state
        .auth
        .authenticate(username, &form.password, &RequestContext::default())
        .await
    {
        Ok(session) => {
            let cookie = session_cookie(session.token, state.secure_cookie);
            (jar.add(cookie), Redirect::to("/admin")).into_response()
        }
        Err(_) => Redirect::to("/admin/login").into_response(),
    }
}

/// Builds the setup view, its timezone select defaulting to the deployment
/// default so the first administrator can accept or change it.
fn setup_view(default_tz: Tz, error: Option<&str>) -> SetupTemplate {
    let default_name = default_tz.name();
    let zones = TZ_VARIANTS
        .iter()
        .map(|tz| TzOption {
            name: tz.name().to_string(),
            selected: tz.name() == default_name,
        })
        .collect();
    SetupTemplate {
        zones,
        error: error.map(|e| e.to_string()),
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

/// Whether an operator may see a settings item: items with no permission are
/// public, otherwise the operator must hold the item's permission.
fn operator_can_see(item: &settings::SettingsItem, perms: &PermissionSet) -> bool {
    match &item.permission {
        None => true,
        Some(p) => perms.allows(p),
    }
}

/// The settings items this operator may see, in registry order. Both the index
/// and the form use this set, so an operator never sees or edits an item their
/// permissions do not allow.
fn visible_settings(
    items: &[settings::SettingsItem],
    perms: &PermissionSet,
) -> Vec<settings::SettingsItem> {
    items
        .iter()
        .filter(|item| operator_can_see(item, perms))
        .cloned()
        .collect()
}

async fn settings_index(Extension(shell): Extension<Shell>) -> Response {
    settings::index(shell)
}

async fn settings_edit(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
) -> Response {
    // Filter first, so an operator cannot open a settings form they lack the
    // permission to see.
    let items = visible_settings(&state.settings, &user.permissions);
    match items
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
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    let items = visible_settings(&state.settings, &user.permissions);
    match items
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
            permission: Some("backend.manage_users".to_string()),
        },
        Resource {
            base_path: "/admin/roles".to_string(),
            nav_label: "Roles".to_string(),
            list: roles_list_config(),
            form: Some(role_form_config()),
            permission: Some("backend.manage_roles".to_string()),
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
            icon: Some("users".to_string()),
            permission: Some("backend.manage_users".to_string()),
            link: Some("/admin/users".to_string()),
            fields: Vec::new(),
        },
        settings::SettingsItem {
            code: "backend.roles".to_string(),
            label: "Roles".to_string(),
            description: "Manage roles and their permissions.".to_string(),
            category: "Users".to_string(),
            order: 20,
            icon: Some("shield".to_string()),
            permission: Some("backend.manage_roles".to_string()),
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
#[template(path = "setup.html")]
struct SetupTemplate {
    zones: Vec<TzOption>,
    error: Option<String>,
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
    active: bool,
    /// Inline SVG for the tab's icon, or empty for a text-only tab. Rendered raw
    /// with `|safe`.
    icon: &'static str,
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

pub(crate) fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "Forbidden").into_response()
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

    fn settings_item(code: &str, permission: Option<&str>) -> settings::SettingsItem {
        settings::SettingsItem {
            code: code.to_string(),
            label: code.to_string(),
            description: String::new(),
            category: "General".to_string(),
            order: 1,
            icon: None,
            permission: permission.map(str::to_string),
            link: None,
            fields: Vec::new(),
        }
    }

    #[test]
    fn settings_visibility_respects_permissions() {
        let items = vec![
            settings_item("public", None),
            settings_item("gated", Some("backend.manage_users")),
        ];

        // An operator without the grant sees only the unpermissioned item.
        let none = PermissionSet::new(false, Vec::<String>::new());
        let codes: Vec<String> = visible_settings(&items, &none)
            .into_iter()
            .map(|i| i.code)
            .collect();
        assert_eq!(codes, ["public"]);

        // Holding the permission reveals the gated item.
        let granted = PermissionSet::new(false, ["backend.manage_users".to_string()]);
        assert_eq!(visible_settings(&items, &granted).len(), 2);

        // A superuser sees everything.
        let superuser = PermissionSet::new(true, Vec::<String>::new());
        assert_eq!(visible_settings(&items, &superuser).len(), 2);
    }

    #[test]
    fn context_sidebar_follows_settings_links() {
        let items = builtin_settings();
        let superuser = PermissionSet::new(true, Vec::<String>::new());
        let sidebar = |path: &str| resolve_nav_context(&[], &items, &superuser, path).0;
        let active_path = |path: &str| -> Option<String> {
            sidebar(path)
                .into_iter()
                .flat_map(|g| g.items)
                .find(|i| i.active)
                .map(|i| i.path)
        };

        // A linked resource, and its sub-pages, resolve to that item as active.
        assert_eq!(active_path("/admin/users").as_deref(), Some("/admin/users"));
        assert_eq!(
            active_path("/admin/roles/42/edit").as_deref(),
            Some("/admin/roles")
        );
        // The settings index shows the sidebar, with nothing active.
        assert!(!sidebar("/admin/settings").is_empty());
        assert_eq!(active_path("/admin/settings"), None);
        // A settings form activates its own item.
        assert_eq!(
            active_path("/admin/settings/backend.roles").as_deref(),
            Some("/admin/roles")
        );
        // The dashboard is not a settings context, so it has no sidebar.
        assert!(sidebar("/admin").is_empty());
    }

    #[test]
    fn active_nav_lights_the_right_tab() {
        let nav = vec![
            NavLink {
                label: "Dashboard".to_string(),
                path: "/admin".to_string(),
                icon: Some("layout-dashboard"),
            },
            NavLink {
                label: "Pages".to_string(),
                path: "/admin/pages".to_string(),
                icon: None,
            },
            NavLink {
                label: "Settings".to_string(),
                path: "/admin/settings".to_string(),
                icon: Some("settings"),
            },
        ];

        // Dashboard lights only on an exact match, never as a prefix of deeper paths.
        assert_eq!(
            active_nav_path(&nav, false, "/admin").as_deref(),
            Some("/admin")
        );
        // A section keeps its own tab active across its sub-pages.
        assert_eq!(
            active_nav_path(&nav, false, "/admin/pages/7/edit").as_deref(),
            Some("/admin/pages")
        );
        // A sibling section that merely shares a prefix does not steal the tab.
        assert_eq!(active_nav_path(&nav, false, "/admin/pages-archive"), None);
        // A screen under no section (reached from the user menu) lights nothing,
        // rather than the root falling back to Dashboard.
        assert_eq!(active_nav_path(&nav, false, "/admin/preferences"), None);
        // The settings context lights Settings, including for a linked resource
        // whose path lives outside /admin/settings.
        assert_eq!(
            active_nav_path(&nav, true, "/admin/users").as_deref(),
            Some("/admin/settings")
        );
        assert_eq!(
            active_nav_path(&nav, true, "/admin/settings").as_deref(),
            Some("/admin/settings")
        );
    }
}
