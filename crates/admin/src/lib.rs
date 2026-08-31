//! Laterite admin: the operator-facing web surface.
//!
//! An Axum router mounted under a configurable path (default `/admin`, set by
//! [`AdminConfig::path`]): a login screen and session cookie verified against
//! `laterite-auth`, and descriptor-driven screens.
//!
//! Screens are **resources**: a module declares a [`Resource`] (a
//! [`list::ListConfig`], optionally a [`form::FormConfig`], a base path, and a
//! menu label), and the framework mounts the list, create, and edit routes and
//! adds it to the menu. This is the extension point that lets an application
//! contribute its own admin screens. The framework's own screens (users, roles)
//! are just built-in resources.
//!
//! An application usually boots through [`Bootstrap`], which loads config,
//! connects, migrates, and serves this router in one call.

pub mod bootstrap;
pub mod form;
mod icons;
pub mod list;
mod plugins;
mod roles;
pub mod settings;
mod sql;
mod users;

pub use bootstrap::{AppConfig, Bootstrap, BootstrapCtx, DEFAULT_ENV_PREFIX};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

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
use laterite_core::Db;
use serde::Deserialize;

/// Typed contribution channels for the framework's admin surfaces, as an
/// extension trait over the generic [`laterite_core::Registry`]. A module
/// contributes its admin screens, permissions, and settings from its `register`;
/// a wrong-type contribution is a compile error, not one silently ignored. The
/// generic `add`/`items` underneath remains for open, plugin-defined points.
pub trait AdminRegistry {
    /// Adds a list/form resource (an admin screen).
    fn add_resource(&mut self, resource: Resource);
    /// Adds a permission, offered in the role editor.
    fn add_permission(&mut self, permission: Permission);
    /// Adds a settings model.
    fn add_settings(&mut self, item: settings::SettingsItem);
}

impl AdminRegistry for laterite_core::Registry {
    fn add_resource(&mut self, resource: Resource) {
        self.add(resource);
    }
    fn add_permission(&mut self, permission: Permission) {
        self.add(permission);
    }
    fn add_settings(&mut self, item: settings::SettingsItem) {
        self.add(item);
    }
}

const SESSION_COOKIE: &str = "laterite_session";

/// Shared state for the admin router. Constructed by [`router`].
#[derive(Clone)]
pub(crate) struct AdminState {
    auth: AuthService,
    db: Db,
    nav: Arc<Vec<NavLink>>,
    settings: Arc<Vec<settings::SettingsItem>>,
    permissions: Arc<Vec<Permission>>,
    /// The URL path the panel is mounted under, without a trailing slash (e.g.
    /// `/admin`). Every route, redirect, link, and the session-cookie scope is
    /// built from it, so one config value moves the whole panel.
    admin_path: Arc<str>,
    secure_cookie: bool,
    timezone: Tz,
    /// The configured application name (the baseline brand). A brand setting
    /// overrides it; see [`AdminState::brand`].
    app_name: String,
    /// The resolved brand name, cached across requests so the brand setting is
    /// not read from the database on every page. Invalidated when the setting is
    /// saved. `None` means "not resolved yet".
    brand_cache: Arc<RwLock<Option<String>>>,
}

impl AdminState {
    #[cfg(test)]
    pub(crate) fn new(auth: AuthService, db: Db) -> Self {
        Self {
            auth,
            db,
            nav: Arc::new(Vec::new()),
            settings: Arc::new(Vec::new()),
            permissions: Arc::new(builtin_permissions()),
            admin_path: Arc::from("/admin"),
            secure_cookie: false,
            timezone: Tz::UTC,
            app_name: "Laterite".to_string(),
            brand_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// The brand name shown across the admin: the [`settings::BrandSetting`]
    /// `app_name` when set, otherwise the configured application name. The
    /// resolved value is cached until [`AdminState::invalidate_brand`] clears it.
    async fn brand(&self) -> String {
        {
            let cached = self.brand_cache.read().unwrap().clone();
            if let Some(name) = cached {
                return name;
            }
        }
        let resolved = match settings::store::load::<settings::BrandSetting>(&self.db).await {
            Ok(brand) if !brand.app_name.trim().is_empty() => brand.app_name,
            _ => self.app_name.clone(),
        };
        *self.brand_cache.write().unwrap() = Some(resolved.clone());
        resolved
    }

    /// Clears the cached brand so the next resolution re-reads the setting.
    fn invalidate_brand(&self) {
        *self.brand_cache.write().unwrap() = None;
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
    /// The application name, shown as the admin brand. This is the baseline; a
    /// `BrandSetting` in the admin overrides it. Typically the configured
    /// `app.name`. Empty falls back to `Laterite`.
    pub app_name: String,
    /// The URL path the panel is mounted under (typically the configured
    /// `backend.path`). A leading slash is added if missing and a trailing slash
    /// is stripped; empty falls back to `/admin`.
    pub path: String,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            secure_cookie: false,
            timezone: "UTC".to_string(),
            app_name: "Laterite".to_string(),
            path: "/admin".to_string(),
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
    /// The admin mount path (e.g. `/admin`), so shared chrome (asset links, the
    /// brand link, sign-out and preferences) builds URLs under the configured
    /// panel path rather than a hardcoded `/admin`.
    pub(crate) base: String,
    /// The brand name shown in the top nav and drawer, resolved once per request
    /// (the brand setting, or the configured application name). See
    /// [`AdminState::brand`].
    brand: String,
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
        base: &str,
        brand: String,
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
            base: base.to_string(),
            brand,
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
            base: "/admin".to_string(),
            brand: "Laterite".to_string(),
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
fn settings_context(
    visible: &[settings::SettingsItem],
    admin_path: &str,
    path: &str,
) -> (bool, Option<String>) {
    let settings_root = format!("{admin_path}/settings");
    if path == settings_root {
        (true, None)
    } else if let Some(code) = path.strip_prefix(&format!("{settings_root}/")) {
        (true, Some(code.to_string()))
    } else {
        // A linked resource: the settings item whose resolved link is the longest
        // prefix of this path owns the context (so /admin/roles/5/edit resolves).
        // Links are stored relative to the admin root, so resolve each here.
        let active = visible
            .iter()
            .filter_map(|i| {
                i.link
                    .as_deref()
                    .map(|link| (format!("{admin_path}{link}"), &i.code))
            })
            .filter(|(link, _)| path == link.as_str() || path.starts_with(&format!("{link}/")))
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
fn active_nav_path(
    nav: &[NavLink],
    admin_path: &str,
    in_settings_context: bool,
    path: &str,
) -> Option<String> {
    if in_settings_context {
        return Some(format!("{admin_path}/settings"));
    }
    nav.iter()
        .filter(|n| {
            path == n.path || (n.path != admin_path && path.starts_with(&format!("{}/", n.path)))
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
    admin_path: &str,
    perms: &PermissionSet,
    path: &str,
) -> (Vec<settings::CategoryView>, Option<String>) {
    let visible = visible_settings(items, perms);
    let (in_context, active) = settings_context(&visible, admin_path, path);
    let sidebar = if in_context {
        settings::sidebar_groups(&visible, admin_path, active.as_deref())
    } else {
        Vec::new()
    };
    let active_nav = active_nav_path(nav, admin_path, in_context, path);
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
    /// The path the resource mounts at, relative to the admin root and starting
    /// with a slash (e.g. `/products`). The framework prepends the configured
    /// admin mount, so a resource never hardcodes it and survives a moved panel.
    /// The list's `edit_base` and the form's `base_path` are resolved the same
    /// way.
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

/// A permission an operator can be granted: a dotted `code`, a human `label`,
/// and a `group` heading it sorts under in the role editor. The framework
/// registers its own (see the built-in grants), and an application registers
/// its permissions through [`router`] so they appear in the editor alongside.
#[derive(Clone)]
pub struct Permission {
    pub code: String,
    pub label: String,
    pub group: String,
}

/// The framework's own permissions, offered in the role editor under a
/// "Backend" group. These gate the built-in Users and Roles screens.
fn builtin_permissions() -> Vec<Permission> {
    vec![
        Permission {
            code: "backend.manage_users".to_string(),
            label: "Manage backend users".to_string(),
            group: "Backend".to_string(),
        },
        Permission {
            code: "backend.manage_roles".to_string(),
            label: "Manage roles".to_string(),
            group: "Backend".to_string(),
        },
        Permission {
            code: "backend.manage_branding".to_string(),
            label: "Manage branding".to_string(),
            group: "Backend".to_string(),
        },
    ]
}

/// The migration sets for every module the admin mounts: the auth schema
/// (users, roles, sessions, access log) and the settings store. Run these
/// before serving [`router`] so its built-in screens have their tables, so an
/// application never has to know which framework modules the admin pulls in.
///
/// An application with its own modules appends their sets:
///
/// ```no_run
/// # async fn f(db: laterite_core::Db) -> Result<(), Box<dyn std::error::Error>> {
/// let mut migrations = laterite_admin::builtin_migrations();
/// // migrations.extend([my_module::migrations()]);
/// laterite_core::migration::run(&db.pool, db.backend, &migrations).await?;
/// # Ok(()) }
/// ```
pub fn builtin_migrations() -> Vec<laterite_core::MigrationSet> {
    builtin_modules().iter().map(|m| m.migrations()).collect()
}

/// The framework's built-in modules, in registration order. `Bootstrap`
/// registers these before an app's own modules, so the admin's tables migrate
/// first.
pub fn builtin_modules() -> Vec<Box<dyn laterite_core::Module>> {
    vec![
        Box::new(laterite_auth::AuthModule),
        Box::new(settings::SettingsModule),
        Box::new(plugins::PluginsModule),
    ]
}

/// Builds the admin router. `app_resources` are the application's own list/form
/// screens; `app_settings` are its settings models; `app_permissions` are the
/// permissions it defines, offered in the role editor alongside the framework's.
/// All are mounted alongside the framework's built-in equivalents.
pub fn router(
    auth: AuthService,
    db: Db,
    app_resources: Vec<Resource>,
    app_settings: Vec<settings::SettingsItem>,
    app_permissions: Vec<Permission>,
    config: AdminConfig,
) -> Router {
    let admin_path = normalize_path(&config.path);

    let mut resources = builtin_resources();
    let mut app_resources = app_resources;
    // Descriptor paths are authored relative to the admin root; resolve them to
    // full paths under the configured mount so routes, the menu, and every link
    // built from them agree.
    for resource in resources.iter_mut().chain(app_resources.iter_mut()) {
        prefix_resource(&admin_path, resource);
    }

    let mut settings = builtin_settings();
    settings.extend(app_settings);
    let mut permissions = builtin_permissions();
    permissions.extend(app_permissions);

    // Main menu (top nav): Dashboard, the application's own sections, then
    // Settings. Built-in Users and Roles are settings items (see the settings
    // menu), not main-menu tabs.
    let mut nav = vec![NavLink {
        label: "Dashboard".to_string(),
        path: admin_path.clone(),
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
        path: format!("{admin_path}/settings"),
        icon: Some("settings"),
    });
    resources.extend(app_resources);

    let app_name = if config.app_name.trim().is_empty() {
        "Laterite".to_string()
    } else {
        config.app_name.clone()
    };
    let state = AdminState {
        auth,
        db,
        nav: Arc::new(nav),
        settings: Arc::new(settings),
        permissions: Arc::new(permissions),
        admin_path: Arc::from(admin_path.as_str()),
        secure_cookie: config.secure_cookie,
        timezone: config.timezone.parse().unwrap_or(Tz::UTC),
        app_name,
        brand_cache: Arc::new(RwLock::new(None)),
    };

    let mut protected = Router::new().route(&admin_path, get(dashboard));
    for resource in &resources {
        protected = protected.merge(mount_resource(resource));
    }
    // The roles screen has a dedicated create/edit form (the permission editor),
    // gated by the same permission as its list.
    protected = protected.merge(guard_with_permission(
        Router::new()
            .route(
                &format!("{admin_path}/roles/new"),
                get(roles::new_form).post(roles::create),
            )
            .route(
                &format!("{admin_path}/roles/{{id}}/edit"),
                get(roles::edit_form).post(roles::update),
            ),
        "backend.manage_roles",
    ));
    // The backend users screen edits a user's per-permission overrides, gated by
    // the same permission as its list.
    protected = protected.merge(guard_with_permission(
        Router::new().route(
            &format!("{admin_path}/users/{{id}}/edit"),
            get(users::edit_form).post(users::update),
        ),
        "backend.manage_users",
    ));
    protected = protected
        .route(&format!("{admin_path}/settings"), get(settings_index))
        .route(
            &format!("{admin_path}/settings/{{code}}"),
            get(settings_edit).post(settings_update),
        )
        .route(
            &format!("{admin_path}/preferences"),
            get(preferences_form).post(preferences_update),
        )
        .route(&format!("{admin_path}/logout"), post(logout));

    protected
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        // Public routes (not covered by the guard above): the login and first-run
        // setup screens and the embedded stylesheet and fonts (needed before
        // authentication).
        .route(
            &format!("{admin_path}/login"),
            get(login_form).post(login_submit),
        )
        .route(
            &format!("{admin_path}/setup"),
            get(setup_form).post(setup_submit),
        )
        .route(&format!("{admin_path}/assets/laterite.css"), get(asset_css))
        .route(&format!("{admin_path}/assets/mark.svg"), get(asset_mark))
        .route(
            &format!("{admin_path}/assets/mark.png"),
            get(asset_mark_png),
        )
        .route(
            &format!("{admin_path}/assets/fonts/{{file}}"),
            get(asset_font),
        )
        .with_state(state)
}

/// Normalises a configured admin path: a single leading slash, no trailing
/// slash, falling back to `/admin` when empty. `manage`, `/manage`, and
/// `/manage/` all become `/manage`. Shared by [`router`] and any caller building
/// the panel's URL (for example a startup banner), so the two never drift.
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        "/admin".to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// Resolves a resource's authored-relative paths (`base_path`, the list's
/// `edit_base`, the form's `base_path`) to full paths under the admin mount.
fn prefix_resource(admin_path: &str, resource: &mut Resource) {
    resource.base_path = format!("{admin_path}{}", resource.base_path);
    if let Some(edit_base) = &mut resource.list.edit_base {
        *edit_base = format!("{admin_path}{edit_base}");
    }
    if let Some(form) = &mut resource.form {
        form.base_path = format!("{admin_path}{}", form.base_path);
    }
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
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            // The stylesheet is embedded in the binary, so it changes only when
            // the binary does. Revalidating on each load keeps a browser from
            // serving a stale copy after an upgrade.
            (header::CACHE_CONTROL, "no-cache"),
        ],
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

    // Gate every route the resource mounts on its permission.
    if let Some(permission) = &resource.permission {
        router = guard_with_permission(router, permission);
    }
    router
}

/// Wraps every route currently in `router` in a permission guard: the auth guard
/// runs first and injects the identity, so this reads it from the request
/// extensions and answers `403 Forbidden` for an operator who lacks `permission`.
/// Shared by resource mounting and the roles permission editor.
fn guard_with_permission(router: Router<AdminState>, permission: &str) -> Router<AdminState> {
    let needed: Arc<str> = Arc::from(permission);
    router.route_layer(middleware::from_fn(
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
    ))
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
            let (sidebar, active_nav) = resolve_nav_context(
                &state.nav,
                &state.settings,
                &state.admin_path,
                &user.permissions,
                &path,
            );
            let brand = state.brand().await;
            let shell = Shell::new(
                &state.admin_path,
                brand,
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
        None => Redirect::to(&format!("{}/login", state.admin_path)).into_response(),
    }
}

async fn login_form(State(state): State<AdminState>) -> Response {
    // A fresh install with no operators goes to first-run setup instead.
    match state.auth.has_any_operator().await {
        Ok(false) => Redirect::to(&format!("{}/setup", state.admin_path)).into_response(),
        Ok(true) => render(LoginTemplate {
            base: state.admin_path.to_string(),
            brand: state.brand().await,
            error: None,
        }),
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
            let cookie = session_cookie(session.token, &state.admin_path, state.secure_cookie);
            (jar.add(cookie), Redirect::to(&state.admin_path)).into_response()
        }
        Err(_) => render(LoginTemplate {
            base: state.admin_path.to_string(),
            brand: state.brand().await,
            error: Some("Invalid username or password.".to_string()),
        }),
    }
}

/// Builds the session cookie, scoped to the admin mount and flagged `Secure`
/// behind HTTPS. Shared by login and first-run setup.
fn session_cookie(token: String, admin_path: &str, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path(admin_path.to_string())
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
        Ok(true) => Redirect::to(&format!("{}/login", state.admin_path)).into_response(),
        Ok(false) => render(setup_view(
            &state.admin_path,
            state.brand().await,
            state.timezone,
            None,
        )),
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
        Ok(true) => return Redirect::to(&format!("{}/login", state.admin_path)).into_response(),
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
            &state.admin_path,
            state.brand().await,
            state.timezone,
            Some("Username, first name, email, and password are all required."),
        ));
    }
    // The setup select always carries a value, but guard against a bad one.
    if tz.parse::<Tz>().is_err() {
        return render(setup_view(
            &state.admin_path,
            state.brand().await,
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
            &state.admin_path,
            state.brand().await,
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
            let cookie = session_cookie(session.token, &state.admin_path, state.secure_cookie);
            (jar.add(cookie), Redirect::to(&state.admin_path)).into_response()
        }
        Err(_) => Redirect::to(&format!("{}/login", state.admin_path)).into_response(),
    }
}

/// Builds the setup view, its timezone select defaulting to the deployment
/// default so the first administrator can accept or change it.
fn setup_view(
    admin_path: &str,
    brand: String,
    default_tz: Tz,
    error: Option<&str>,
) -> SetupTemplate {
    let default_name = default_tz.name();
    let zones = TZ_VARIANTS
        .iter()
        .map(|tz| TzOption {
            name: tz.name().to_string(),
            selected: tz.name() == default_name,
        })
        .collect();
    SetupTemplate {
        base: admin_path.to_string(),
        brand,
        zones,
        error: error.map(|e| e.to_string()),
    }
}

async fn logout(State(state): State<AdminState>, jar: CookieJar) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let _ = state.auth.logout(cookie.value()).await;
    }
    let removal = Cookie::build((SESSION_COOKIE, ""))
        .path(state.admin_path.to_string())
        .build();
    (
        jar.remove(removal),
        Redirect::to(&format!("{}/login", state.admin_path)),
    )
        .into_response()
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
        Ok(()) => {
            Redirect::to(&format!("{}/preferences?saved=1", state.admin_path)).into_response()
        }
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
            base_path: "/users".to_string(),
            nav_label: "Backend Users".to_string(),
            list: backend_users_list_config(),
            form: None,
            permission: Some("backend.manage_users".to_string()),
        },
        Resource {
            base_path: "/roles".to_string(),
            nav_label: "Roles".to_string(),
            list: roles_list_config(),
            // The create/edit form is the dedicated permission editor (see the
            // `roles` module), mounted separately, not the generic form.
            form: None,
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
            link: Some("/users".to_string()),
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
            link: Some("/roles".to_string()),
            fields: Vec::new(),
        },
        settings::brand::settings_item(),
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
        // Rows link to the per-user permission editor; users are created from the
        // CLI or first-run setup, so no "New" screen here.
        edit_base: Some("/users".to_string()),
        creatable: false,
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
        edit_base: Some("/roles".to_string()),
        creatable: true,
    }
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    /// The admin mount path, so pre-auth asset and form URLs match the panel.
    base: String,
    brand: String,
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
    /// The admin mount path, so pre-auth asset and form URLs match the panel.
    base: String,
    brand: String,
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

    /// A fresh test database with no migrations applied, the blank slate an
    /// application starts from before it runs its migration set. Hold the guard
    /// for the test's lifetime.
    async fn empty_db() -> (Db, laterite_core::testing::TestGuard) {
        laterite_core::testing::connect_test(&[]).await
    }

    #[tokio::test]
    async fn builtin_migrations_create_the_admin_tables() {
        let (db, _guard) = empty_db().await;
        laterite_core::migration::run(&db.pool, db.backend, &builtin_migrations())
            .await
            .unwrap();
        // A table from each bundled module exists, so an app that only ran
        // builtin_migrations has everything the admin's screens need. A no-row
        // probe succeeds only if the table exists, portably on every backend.
        for table in ["backend_users", "settings"] {
            let probe = sqlx::query(&format!("select 1 from {table} where 1 = 0"))
                .fetch_optional(&db.pool)
                .await;
            assert!(
                probe.is_ok(),
                "{table} should exist after builtin_migrations"
            );
        }
    }

    #[tokio::test]
    async fn brand_setting_overrides_config_and_blank_falls_back() {
        let (db, _guard) = laterite_core::testing::connect_test(&[settings::migrations()]).await;
        let state = AdminState {
            auth: AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db: db.clone(),
            nav: Arc::new(Vec::new()),
            settings: Arc::new(Vec::new()),
            permissions: Arc::new(builtin_permissions()),
            admin_path: Arc::from("/admin"),
            secure_cookie: false,
            timezone: Tz::UTC,
            app_name: "Configured Name".to_string(),
            brand_cache: Arc::new(RwLock::new(None)),
        };

        // With no brand setting, the configured application name is the brand.
        assert_eq!(state.brand().await, "Configured Name");

        // A brand setting overrides the configured name (cache re-reads after
        // invalidation).
        settings::store::save(
            &db,
            &settings::BrandSetting {
                app_name: "Acme Corp".to_string(),
            },
        )
        .await
        .unwrap();
        state.invalidate_brand();
        assert_eq!(state.brand().await, "Acme Corp");

        // A blank brand setting falls back to the configured name.
        settings::store::save(
            &db,
            &settings::BrandSetting {
                app_name: "   ".to_string(),
            },
        )
        .await
        .unwrap();
        state.invalidate_brand();
        assert_eq!(state.brand().await, "Configured Name");
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
        let sidebar = |path: &str| resolve_nav_context(&[], &items, "/admin", &superuser, path).0;
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
            active_nav_path(&nav, "/admin", false, "/admin").as_deref(),
            Some("/admin")
        );
        // A section keeps its own tab active across its sub-pages.
        assert_eq!(
            active_nav_path(&nav, "/admin", false, "/admin/pages/7/edit").as_deref(),
            Some("/admin/pages")
        );
        // A sibling section that merely shares a prefix does not steal the tab.
        assert_eq!(
            active_nav_path(&nav, "/admin", false, "/admin/pages-archive"),
            None
        );
        // A screen under no section (reached from the user menu) lights nothing,
        // rather than the root falling back to Dashboard.
        assert_eq!(
            active_nav_path(&nav, "/admin", false, "/admin/preferences"),
            None
        );
        // The settings context lights Settings, including for a linked resource
        // whose path lives outside /admin/settings.
        assert_eq!(
            active_nav_path(&nav, "/admin", true, "/admin/users").as_deref(),
            Some("/admin/settings")
        );
        assert_eq!(
            active_nav_path(&nav, "/admin", true, "/admin/settings").as_deref(),
            Some("/admin/settings")
        );
    }
}
