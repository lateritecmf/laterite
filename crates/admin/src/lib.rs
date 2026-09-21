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

mod audit;
pub mod bootstrap;
mod bulk;
mod clientip;
mod error;
mod export;
pub mod field;
pub mod form;
pub mod html;
pub mod http_cache;
mod icons;
pub mod list;
pub mod persist;
pub mod picker;
pub mod plugins;
mod roles;
mod routemap;
pub mod routes;
mod session;
pub mod settings;
mod sql;
mod upload;
mod users;

/// The axum a contributed route must build against.
///
/// A [`Screen`](routes::Screen) or [`PublicRoute`](routes::PublicRoute) returns an
/// `axum::Router`, so a plugin needs the type. Take it from here and never declare
/// axum in a plugin manifest: two majors linked side by side make `Router` a
/// different type from `Router`, and the diagnostic for that is famously unhelpful.
/// Re-exported so this crate's manifest is the only place the version is chosen.
pub use axum;

pub use bootstrap::{AppConfig, Bootstrap, BootstrapCtx, DEFAULT_ENV_PREFIX};
pub use error::AdminError;
pub use session::{FlashLevel, SessionHandle};
pub use upload::VerifiedUpload;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use askama::Template;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Extension, Form, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono_tz::{Tz, TZ_VARIANTS};
use laterite_auth::{AuthService, AuthenticatedUser, NewOperator, PermissionSet, RequestContext};
use laterite_core::{t, CatalogStore, Db, Text, Translator};
use serde::{Deserialize, Serialize};

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
    /// Adds a reference-picker source, callable by a picker field's endpoints.
    fn add_picker_source(&mut self, source: picker::PickerSourceReg);
    /// Adds a form persister, selectable by a form descriptor's `persist` key.
    fn add_persister(&mut self, persister: persist::PersisterReg);
    /// Adds a field type, nameable by any form or settings descriptor.
    fn add_field_type(&mut self, field_type: field::FieldTypeReg);
    /// Mounts a screen of this module's own: any routes descriptors cannot
    /// express, inside the admin with its session, permission and error handling.
    fn add_screen(&mut self, screen: routes::ScreenReg);
    /// Mounts a route outside the admin, at a literal path: robots.txt, a
    /// sitemap, a feed, a webhook receiver.
    fn add_public_route(&mut self, route: routes::PublicRouteReg);
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
    fn add_picker_source(&mut self, source: picker::PickerSourceReg) {
        self.add(source);
    }
    fn add_screen(&mut self, screen: routes::ScreenReg) {
        self.add(screen);
    }
    fn add_public_route(&mut self, route: routes::PublicRouteReg) {
        self.add(route);
    }
    fn add_persister(&mut self, persister: persist::PersisterReg) {
        self.add(persister);
    }
    fn add_field_type(&mut self, field_type: field::FieldTypeReg) {
        self.add(field_type);
    }
}

const SESSION_COOKIE: &str = "laterite_session";
/// The long-lived "stay signed in" credential. Separate from the session cookie
/// so the session itself stays short and the credential can be rotated and
/// revoked on its own.
const REMEMBER_COOKIE: &str = "laterite_remember";

/// The icon sprite's registry key. One request, cached forever, in place of a
/// few hundred bytes of repeated markup per icon per render.
pub(crate) const ICON_SPRITE: &str = "icons.svg";

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
    /// Networks whose `X-Forwarded-For` is believed. Parsed once at boot.
    trusted_proxies: Arc<Vec<ipnet::IpNet>>,
    /// The framework's icon set plus whatever modules contributed.
    icons: Arc<laterite_core::icons::IconSet>,
    /// Where the sprite is served, content-named. Resolved once at boot.
    sprite_url: Arc<str>,
    /// The public origin for the CSRF origin check (see [`AdminConfig::origin`]).
    origin: Arc<str>,
    timezone: Tz,
    /// The deployment default UI locale (a base tag like `en`), the last stop in a
    /// request's locale chain before the English source. An operator's own
    /// preference overrides it.
    default_locale: Arc<str>,
    /// The shared message catalogs, resolved once at boot and looked up per request
    /// through the [`Translator`]. Empty until module catalogs are loaded.
    catalogs: Arc<CatalogStore>,
    /// The configured application name (the baseline brand). A brand setting
    /// overrides it; see [`AdminState::brand`].
    app_name: String,
    /// The resolved brand name, cached across requests so the brand setting is
    /// not read from the database on every page. Invalidated when the setting is
    /// saved. `None` means "not resolved yet".
    brand_cache: Arc<RwLock<Option<String>>>,
    /// Each registered asset's content-named path. See [`asset_urls`].
    asset_urls: Arc<AssetUrls>,
    /// The field-type registry (built-ins plus contributions): resolves a form
    /// descriptor's type key to its rendering + behaviour. See [`field`].
    field_types: Arc<field::FieldRegistry>,
    /// The column-type registry: resolves a list column's type key to its cell
    /// rendering. Sibling to `field_types`; shares the override resolver.
    column_types: Arc<list::ColumnRegistry>,
    /// Contributions the framework does not consume, readable by a module's own
    /// routes through `RouteCtx`.
    plugin_defined: Arc<laterite_core::Registry>,
    /// Embedded admin assets served under `{admin}/assets/` (see [`AdminAsset`]).
    assets: Arc<AssetRegistry>,
    /// The reference-picker source registry: resolves a picker field's `source`
    /// key to the domain service its endpoints call. See [`picker`].
    pickers: Arc<picker::PickerRegistry>,
    /// The admin view-override resolver, default [`field::NoOverrides`] (the
    /// compiled path). A theme layer injects a runtime-template-backed resolver
    /// so users can override a field's presentation from outside the plugin.
    overrides: Arc<dyn field::OverrideResolver>,
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
            trusted_proxies: Arc::new(Vec::new()),
            icons: Arc::new(laterite_core::icons::IconSet::new()),
            sprite_url: Arc::from(""),
            admin_path: Arc::from("/admin"),
            secure_cookie: false,
            origin: Arc::from(""),
            timezone: Tz::UTC,
            default_locale: Arc::from("en"),
            catalogs: Arc::new(CatalogStore::default()),
            app_name: "Laterite".to_string(),
            brand_cache: Arc::new(RwLock::new(None)),
            field_types: Arc::new(field::builtin_registry()),
            column_types: Arc::new(list::builtin_column_registry()),
            plugin_defined: Arc::new(laterite_core::Registry::new()),
            assets: Arc::new(builtin_assets()),
            asset_urls: Arc::new(asset_urls(&builtin_assets())),
            pickers: Arc::new(picker::PickerRegistry::new()),
            overrides: Arc::new(field::NoOverrides),
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
///
/// Non-exhaustive: build one with [`Default`] and set fields, so a field added
/// in a later release is additive.
#[derive(Clone)]
#[non_exhaustive]
pub struct AdminConfig {
    /// Set the `Secure` attribute on the session cookie. Enable behind HTTPS in
    /// production; leave off for plain-HTTP local development.
    pub secure_cookie: bool,
    /// Networks whose `X-Forwarded-For` may be believed, as CIDR ranges. Empty
    /// trusts nothing and records the peer address. See
    /// [`laterite_core::config::BackendConfig::trusted_proxies`].
    pub trusted_proxies: Vec<String>,
    /// Default display timezone for the admin (an IANA name like `Asia/Kolkata`).
    /// Storage is UTC; this only affects rendering. Invalid or empty falls back
    /// to UTC. An operator's own preference overrides it (later).
    pub timezone: String,
    /// Default UI locale for the admin (a base tag like `en` or `de`). An
    /// operator's own preference and the request's `Accept-Language` override it;
    /// a locale with no loaded catalog, or an empty value, falls back to `en`.
    pub locale: String,
    /// The application name, shown as the admin brand. This is the baseline; a
    /// `BrandSetting` in the admin overrides it. Typically the configured
    /// `app.name`. Empty falls back to `Laterite`.
    pub app_name: String,
    /// The URL path the panel is mounted under (typically the configured
    /// `backend.path`). A leading slash is added if missing and a trailing slash
    /// is stripped; empty falls back to `/admin`.
    pub path: String,
    /// The public origin (scheme://host[:port], no trailing slash) the panel is
    /// served from, for the CSRF origin check. Typically the configured
    /// `app.url`; empty falls back to the request `Host` (dev only), so set it in
    /// production.
    pub origin: String,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            secure_cookie: false,
            trusted_proxies: Vec::new(),
            timezone: "UTC".to_string(),
            locale: "en".to_string(),
            app_name: "Laterite".to_string(),
            path: "/admin".to_string(),
            origin: String::new(),
        }
    }
}

#[derive(Clone)]
struct NavLink {
    label: Text,
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
    /// The translator for this request's locale (resolved like `tz`), used by
    /// `shell.t` / `shell.tt` in templates. A source with no catalog entry falls
    /// back to itself.
    i18n: Translator,
    /// The context sidebar for the current section, resolved once per request
    /// from the path (see [`resolve_nav_context`]). Empty means no sidebar.
    /// `base.html` renders it, so any screen in a settings context shows it.
    sidebar: Vec<settings::CategoryView>,
    /// The icon set, for chrome that renders one outside a nav entry.
    pub(crate) icon_set: Arc<laterite_core::icons::IconSet>,
    /// Where the sprite lives, so chrome can reference a glyph rather than
    /// carry its drawing instructions.
    pub(crate) sprite_url: Arc<str>,
    /// The site's own root, so the chrome can offer a way out to the front of
    /// the site an operator is administering. The configured `app.url` when set,
    /// else derived from the bind address.
    site_url: String,
    /// Each asset's content-named path, so chrome in `base.html` links assets
    /// through [`Shell::asset`] rather than by a stable path the browser would
    /// be entitled to keep.
    asset_urls: Arc<AssetUrls>,
    /// The current session's CSRF token, auto-injected into every rendered form
    /// (a hidden field) and into HTMX requests (a header), so a mutating request
    /// carries it without the handler doing anything. See [`session`].
    pub(crate) csrf_token: String,
    /// Flash messages to show once on this render, taken from the session on a
    /// full-page GET (redirect-after-POST delivers them here) and localized in this
    /// request's locale. See [`session`].
    pub(crate) flash: Vec<FlashLine>,
    /// Per-page widget assets (field/column scripts and styles) for this render,
    /// deduped and emitted in the head after core `laterite.js`. See
    /// [`page_assets`].
    pub(crate) assets: Vec<PageAsset>,
}

/// One per-page asset to load: a resolved URL and whether it is a stylesheet.
#[derive(Clone)]
pub(crate) struct PageAsset {
    pub(crate) url: String,
    pub(crate) css: bool,
}

/// A flash message localized for this render: the session's stored `Text` resolved
/// to a display string in the operator's locale. `base.html` renders it.
#[derive(Clone)]
pub(crate) struct FlashLine {
    pub(crate) level: session::FlashLevel,
    pub(crate) text: String,
}

impl Shell {
    #[allow(clippy::too_many_arguments)]
    fn new(
        base: &str,
        brand: String,
        nav: &[NavLink],
        user: &AuthenticatedUser,
        default_tz: Tz,
        sidebar: Vec<settings::CategoryView>,
        active_nav: Option<&str>,
        csrf_token: String,
        flash: Vec<session::Flash>,
        i18n: Translator,
        asset_urls: Arc<AssetUrls>,
        site_url: String,
        icon_set: Arc<laterite_core::icons::IconSet>,
        sprite_url: Arc<str>,
    ) -> Self {
        let full_name = user.user.full_name();
        let initial = full_name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        // Localize the flashes and nav labels here, where the request translator is
        // in hand, before it is moved into the shell.
        let flash = flash
            .into_iter()
            .map(|f| FlashLine {
                level: f.level,
                text: i18n.t(&f.text),
            })
            .collect();
        let nav = nav
            .iter()
            .map(|n| NavView {
                label: i18n.t(&n.label),
                path: n.path.clone(),
                active: active_nav == Some(n.path.as_str()),
                icon: n
                    .icon
                    .map(|name| icons::svg(&icon_set, &sprite_url, Some(name)))
                    .unwrap_or_default(),
            })
            .collect();
        Shell {
            asset_urls,
            icon_set,
            sprite_url,
            site_url,
            base: base.to_string(),
            brand,
            nav,
            full_name,
            initial,
            tz: resolve_display_tz(user.user.timezone.as_deref(), default_tz),
            i18n,
            sidebar,
            csrf_token,
            flash,
            assets: Vec::new(),
        }
    }

    /// The site's own root, for the chrome's link out to the front of the site.
    pub(crate) fn site_url(&self) -> &str {
        &self.site_url
    }

    /// Inline SVG for a named icon, for chrome that is not a nav item.
    pub(crate) fn icon(&self, name: &str) -> String {
        icons::svg(&self.icon_set, &self.sprite_url, Some(name))
    }

    /// The URL for a built-in asset, named by a digest of its bytes. Templates
    /// call `{{ shell.asset("laterite.css") }}`. An unregistered key yields the
    /// plain path, which still serves (revalidating rather than immutable) and
    /// trips a debug assertion.
    pub(crate) fn asset(&self, key: &str) -> String {
        match self.asset_urls.get(key) {
            Some(path) => format!("{}/assets/{path}", self.base),
            None => {
                debug_assert!(false, "asset key `{key}` is not registered");
                format!("{}/assets/{key}", self.base)
            }
        }
    }

    /// Localizes a source string in this request's locale, falling back to the
    /// source itself. Templates call `{{ shell.t("Save") }}`.
    pub(crate) fn t(&self, source: &str) -> String {
        self.i18n.t(&Text::dynamic(source))
    }

    /// Localizes a prebuilt message (a `t!`/`tn!` value, a descriptor label, a
    /// flash) into this request's locale.
    pub(crate) fn tt(&self, text: &Text) -> String {
        self.i18n.t(text)
    }

    /// The request's translator, for a field type that localizes strings of its
    /// own (a repeater's sub-field labels) rather than only the label it is given.
    pub(crate) fn i18n(&self) -> &Translator {
        &self.i18n
    }

    /// The active locale (most specific in the chain), for `<html lang>`.
    pub(crate) fn locale(&self) -> &str {
        self.i18n.locale()
    }

    /// Localizes a source string with integer `{name}` arguments, for a template
    /// (`{{ shell.tf("Page {n} of {m}", [("n", page), ("m", pages)]) }}`). Nested-`Text`
    /// arguments are built in Rust with `t!` and rendered with `tt`.
    pub(crate) fn tf(&self, source: &str, args: &[(&'static str, i64)]) -> String {
        let mut text = Text::dynamic(source);
        for (name, value) in args {
            text = text.arg(*name, *value);
        }
        self.i18n.t(&text)
    }

    /// Like [`Shell::tf`], but with string `{name}` arguments, for a template
    /// (`{{ shell.tfs("Signed in as {name}", [("name", user)]) }}`).
    pub(crate) fn tfs(&self, source: &str, args: &[(&'static str, &str)]) -> String {
        let mut text = Text::dynamic(source);
        for (name, value) in args {
            text = text.arg(*name, *value);
        }
        self.i18n.t(&text)
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Shell {
            asset_urls: Arc::new(asset_urls(&builtin_assets())),
            icon_set: Arc::new(laterite_core::icons::IconSet::new()),
            sprite_url: Arc::from("/admin/assets/icons.svg"),
            site_url: "http://localhost".to_string(),
            base: "/admin".to_string(),
            brand: "Laterite".to_string(),
            nav: Vec::new(),
            full_name: "Test Operator".to_string(),
            initial: "T".to_string(),
            tz: Tz::UTC,
            i18n: Translator::new("en"),
            sidebar: Vec::new(),
            csrf_token: "test-csrf-token".to_string(),
            flash: Vec::new(),
            assets: Vec::new(),
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
    if path == format!("{admin_path}/settings") {
        return (true, None);
    }
    // A settings screen mounts in its module's namespace, not under /settings,
    // so the context is found by matching the resolved paths rather than by
    // stripping a prefix. A linked item (a resource in the settings menu) owns
    // the context across its sub-pages, so /admin/roles/5/edit still resolves;
    // the longest match wins.
    let own = visible
        .iter()
        .filter(|i| i.link.is_none())
        .find(|i| path == format!("{admin_path}{}", i.route()))
        .map(|i| i.code.clone());
    if own.is_some() {
        return (true, own);
    }
    let linked = visible
        .iter()
        .filter_map(|i| {
            i.link
                .as_deref()
                .map(|link| (format!("{admin_path}{link}"), &i.code))
        })
        .filter(|(link, _)| path == link.as_str() || path.starts_with(&format!("{link}/")))
        .max_by_key(|(link, _)| link.len())
        .map(|(_, code)| code.clone());
    (linked.is_some(), linked)
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
    tr: &Translator,
    icons: icons::Icons<'_>,
) -> (Vec<settings::CategoryView>, Option<String>) {
    let visible = visible_settings(items, perms);
    let (in_context, active) = settings_context(&visible, admin_path, path);
    let sidebar = if in_context {
        settings::sidebar_groups(&visible, admin_path, active.as_deref(), tr, icons)
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

/// Whether a timezone is offered in the picker's zone list.
fn zone_offered(name: &str) -> bool {
    !matches!(name, "Asia/Jerusalem" | "Asia/Tel_Aviv")
}

/// The base language of a tag (`kn-IN` -> `kn`), lowercased. Catalogs are keyed by
/// base language.
fn base_lang(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or(tag)
        .to_ascii_lowercase()
}

/// Whether resolution can serve `base`: English (the source, always reachable), a
/// locale with a loaded catalog, or the QA pseudo-locale (selectable via config or
/// `Accept-Language`, though never shown in the picker). The serveable set is the
/// deployment's loaded catalogs, so shipping a `de.po` makes `de` resolve with no
/// code change.
fn is_serveable(base: &str, serveable: &[String]) -> bool {
    base == "en" || base == laterite_core::PSEUDO_LOCALE || serveable.iter().any(|l| l == base)
}

/// The deployment default locale from config, normalized to a base tag the running
/// catalogs can serve, else `en`.
fn default_locale(configured: &str, serveable: &[String]) -> String {
    let base = base_lang(configured);
    if is_serveable(&base, serveable) {
        base
    } else {
        "en".to_string()
    }
}

/// The locales the language picker offers: English (the source, always) then every
/// locale with a loaded catalog, sorted. The pseudo-locale is resolvable but never
/// offered here.
fn offered_locales(catalogs: &CatalogStore) -> Vec<String> {
    let mut out = vec!["en".to_string()];
    out.extend(catalogs.locales());
    out
}

/// A locale's display name for the picker. The framework names only its own source
/// language (English); every other locale shows by its tag. A locale's own name
/// (endonym) is data that belongs with the catalog that provides the locale, not a
/// table of specific languages the framework would otherwise have to hold for all.
fn locale_name(code: &str) -> String {
    if code == "en" {
        "English".to_string()
    } else {
        code.to_string()
    }
}

/// The tags in an `Accept-Language` header, most-preferred first. Each entry's
/// optional `;q=` weight orders them (absent means `1.0`); `*` is dropped.
fn parse_accept_language(header: &str) -> Vec<String> {
    let mut items: Vec<(f32, String)> = header
        .split(',')
        .filter_map(|part| {
            let mut bits = part.split(';');
            let tag = bits.next()?.trim();
            if tag.is_empty() || tag == "*" {
                return None;
            }
            let q = bits
                .find_map(|p| p.trim().strip_prefix("q="))
                .and_then(|q| q.parse::<f32>().ok())
                .unwrap_or(1.0);
            Some((q, tag.to_string()))
        })
        .collect();
    // Stable sort by weight descending, so equal weights keep header order.
    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    items.into_iter().map(|(_, tag)| tag).collect()
}

/// Builds this request's locale fallback chain, most specific first and always
/// ending at `en`. Candidates, in order: the operator's stored locale, the
/// `Accept-Language` tags, the deployment default. Each is normalized to a base
/// language the running catalogs can serve; unserveable tags are skipped and none
/// repeats.
fn resolve_locale_chain(
    preference: Option<&str>,
    accept_language: Option<&str>,
    default: &str,
    serveable: &[String],
) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let consider = |tag: &str, chain: &mut Vec<String>| {
        let base = base_lang(tag);
        if is_serveable(&base, serveable) && !chain.contains(&base) {
            chain.push(base);
        }
    };
    if let Some(pref) = preference {
        consider(pref, &mut chain);
    }
    if let Some(header) = accept_language {
        for tag in parse_accept_language(header) {
            consider(&tag, &mut chain);
        }
    }
    consider(default, &mut chain);
    // Guarantee the source language is reachable, appended only if not already
    // signaled earlier (so an explicit en preference keeps its position).
    consider("en", &mut chain);
    chain
}

/// A translator for a pre-auth screen (login, setup): no operator preference exists
/// yet, so the chain is the request's `Accept-Language`, then the deployment default,
/// then `en`, over the shared catalogs.
fn pre_auth_translator(state: &AdminState, headers: &HeaderMap) -> Translator {
    let accept = headers.get("accept-language").and_then(|v| v.to_str().ok());
    let serveable = state.catalogs.locales();
    let chain = resolve_locale_chain(None, accept, &state.default_locale, &serveable);
    Translator::with_chain(chain, state.catalogs.clone())
}

/// An admin resource: a list screen, optionally with a create/edit form, mounted
/// under `base_path` and shown in the menu as `nav_label`.
///
/// Built with [`Resource::new`] plus its builder methods. Non-exhaustive because
/// this is the descriptor a plugin author writes and the one most likely to grow:
/// a struct literal outside this crate would freeze its field set at 1.0, so the
/// framework could never learn anything new about a resource.
#[derive(Serialize)]
#[non_exhaustive]
pub struct Resource {
    /// The path the resource mounts at, relative to the admin root and starting
    /// with a slash (e.g. `/products`). The framework prepends the configured
    /// admin mount, so a resource never hardcodes it and survives a moved panel.
    /// The list's `edit_base` and the form's `base_path` are resolved the same
    /// way.
    pub base_path: String,
    /// The menu label, localized at render. Serde stays a plain string.
    pub nav_label: Text,
    pub list: list::ListConfig,
    pub form: Option<form::FormConfig>,
    /// The permission an operator must hold to reach any of the resource's
    /// routes. A dotted string gates every route the resource mounts, and an
    /// operator who lacks it receives `403 Forbidden`; superusers pass regardless.
    /// `None` leaves the resource open to any signed-in operator and is allowed
    /// only for a read-only resource: a resource with a create/edit `form` must
    /// declare a permission, or boot aborts (see [`mount_resource`]).
    pub permission: Option<String>,
}

/// A permission an operator can be granted: a dotted `code`, a human `label`,
/// and a `group` heading it sorts under in the role editor. The framework
/// registers its own (see the built-in grants), and an application registers
/// its permissions through [`router`] so they appear in the editor alongside.
#[derive(Clone, Serialize)]
#[non_exhaustive]
pub struct Permission {
    pub code: String,
    /// The permission's display label and group heading, localized at render.
    pub label: Text,
    pub group: Text,
    /// The built-in roles that hold this permission. Empty means
    /// [`ROLE_ADMIN`] alone.
    pub default_roles: Vec<String>,
}

pub use laterite_auth::{ROLE_ADMIN, ROLE_EDITOR};

impl Permission {
    /// A permission an operator can be granted, held by [`ROLE_ADMIN`] until it
    /// names other roles with [`Permission::roles`].
    pub fn new(code: impl Into<String>, label: impl Into<Text>, group: impl Into<Text>) -> Self {
        Self {
            code: code.into(),
            label: label.into(),
            group: group.into(),
            default_roles: Vec::new(),
        }
    }

    /// The built-in roles that hold this permission by default. Naming
    /// [`ROLE_EDITOR`] alone keeps it out of [`ROLE_ADMIN`].
    pub fn roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.default_roles = roles.into_iter().map(Into::into).collect();
        self
    }
}

/// The framework's roles, with the permissions the registry says they hold.
///
/// A permission that names no role belongs to [`ROLE_ADMIN`] alone: an
/// undeclared permission reaching only the administrator is the safe reading,
/// and the role editor shows the author it is not reaching editors.
pub fn system_roles(contributed: &[Permission]) -> Vec<laterite_auth::store::SystemRole<'static>> {
    // The framework's own permissions belong in its own roles, and `router` adds
    // them after this runs, so take them from the same source it does.
    let permissions: Vec<Permission> = builtin_permissions()
        .into_iter()
        .chain(contributed.iter().cloned())
        .collect();
    let held = |role: &str| -> Vec<String> {
        permissions
            .iter()
            .filter(|p| {
                if p.default_roles.is_empty() {
                    role == ROLE_ADMIN
                } else {
                    p.default_roles.iter().any(|r| r == role)
                }
            })
            .map(|p| p.code.clone())
            .collect()
    };
    vec![
        laterite_auth::store::SystemRole {
            code: ROLE_ADMIN,
            name: "Administrator",
            description: "Administers the panel itself: operators, roles and settings.",
            permissions: held(ROLE_ADMIN),
        },
        laterite_auth::store::SystemRole {
            code: ROLE_EDITOR,
            name: "Editor",
            description: "Works with the content the application declares.",
            permissions: held(ROLE_EDITOR),
        },
    ]
}

impl Resource {
    /// A read-only resource: a list at `base_path`, in the menu as `nav_label`.
    ///
    /// A create/edit form is added with [`Resource::form`], which also requires
    /// [`Resource::permission`]: an unguarded write resource aborts boot.
    pub fn new(
        base_path: impl Into<String>,
        nav_label: impl Into<Text>,
        list: list::ListConfig,
    ) -> Self {
        Self {
            base_path: base_path.into(),
            nav_label: nav_label.into(),
            list,
            form: None,
            permission: None,
        }
    }

    /// Adds the create/edit form. A resource with one must also declare a
    /// permission, or boot aborts.
    pub fn form(mut self, form: form::FormConfig) -> Self {
        self.form = Some(form);
        self
    }

    /// The permission gating every route this resource mounts.
    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }
}

/// The framework's own permissions, offered in the role editor under a
/// "Backend" group. These gate the built-in Users and Roles screens.
fn builtin_permissions() -> Vec<Permission> {
    // Each names no role, so each belongs to the administrator alone: every one
    // of them administers the panel rather than its content.
    vec![
        Permission::new("backend.manage_users", "Manage backend users", "Backend"),
        Permission::new("backend.manage_roles", "Manage roles", "Backend"),
        Permission::new("backend.manage_branding", "Manage branding", "Backend"),
        Permission::new(plugins::MANAGE_PERMISSION, "Manage plugins", "Backend"),
        Permission::new("backend.view_audit_log", "View the audit log", "Backend"),
    ]
}

/// Every source string in the framework's built-in admin descriptors: resource nav
/// labels and their list titles and column headers, settings items and their fields,
/// and permission labels and groups. `lat i18n extract` folds these into the admin
/// catalog. Collected through the serde source walk, so it never drifts as new label
/// fields are added.
pub fn descriptor_sources() -> Vec<String> {
    let mut out = Vec::new();
    out.extend(laterite_core::collect_sources(&builtin_resources()));
    out.extend(laterite_core::collect_sources(&builtin_settings()));
    out.extend(laterite_core::collect_sources(&builtin_permissions()));
    out.sort();
    out.dedup();
    out
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
/// Everything the modules and the application contribute to the admin, in one
/// value.
///
/// Build it with the fields you mean and leave the rest to `Default`:
///
/// ```
/// # use laterite_admin::Contributions;
/// let contributions = Contributions {
///     permissions: Vec::new(),
///     ..Default::default()
/// };
/// ```
///
/// It exists so that a new kind of contribution is a new field rather than a new
/// argument. The nine of them were positional before, and a run of interchangeable
/// `Vec::new()` placeholders is a mistake the compiler cannot catch: swap two and
/// it still builds. Naming them removes that whole class of error, and ending a
/// literal with `..Default::default()` means a later field does not break the
/// call.
#[derive(Default)]
pub struct Contributions {
    /// List and form screens, mounted beside the framework's own.
    pub resources: Vec<Resource>,
    /// Settings models, offered in the settings area.
    pub settings: Vec<settings::SettingsItem>,
    /// Permissions defined by the application, offered in the role editor.
    pub permissions: Vec<Permission>,
    /// Sources a reference field searches and resolves against.
    pub picker_sources: Vec<picker::PickerSourceReg>,
    /// Named write handlers a form can select.
    pub persisters: Vec<persist::PersisterReg>,
    /// Listeners run around every save and delete.
    pub listeners: Vec<laterite_core::ModelListenerReg>,
    /// Column types a list cell can render through.
    /// Field types a module contributes, offered to every form and settings
    /// screen beside the built-ins.
    pub field_types: Vec<field::FieldTypeReg>,
    pub column_types: Vec<list::ColumnTypeReg>,
    /// Icons a module contributes, for glyphs the framework's curated set does
    /// not carry. Each is namespaced to the contributing module.
    pub icons: Vec<(laterite_core::ModuleId, laterite_core::icons::IconReg)>,
    /// Screens a module mounts itself, for anything that is not a list or form.
    pub screens: Vec<routes::ScreenReg>,
    /// Routes mounted outside the admin, at literal paths.
    pub public_routes: Vec<routes::PublicRouteReg>,
    /// Everything the framework itself does not consume: the contribution types
    /// modules define for each other. Kept whole so a contributed route can read
    /// them through [`routes::RouteCtx::contributions`], which is what makes a
    /// module's own extension point usable by another module.
    pub plugin_defined: Arc<laterite_core::Registry>,
}

pub fn router(
    auth: AuthService,
    db: Db,
    contributions: Contributions,
    config: AdminConfig,
    catalogs: Arc<CatalogStore>,
) -> Router {
    let Contributions {
        resources: app_resources,
        settings: app_settings,
        permissions: app_permissions,
        picker_sources: app_picker_sources,
        persisters: app_persisters,
        listeners: app_listeners,
        field_types: app_field_types,
        column_types: app_column_types,
        icons: app_icons,
        screens: app_screens,
        public_routes: app_public,
        plugin_defined,
    } = contributions;
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
    // The codes a column may require; `list::check` refuses one nobody registered.
    let permission_codes: std::collections::HashSet<String> =
        permissions.iter().map(|p| p.code.clone()).collect();

    // Main menu (top nav): Dashboard, the application's own sections, then
    // Settings. Built-in Users and Roles are settings items (see the settings
    // menu), not main-menu tabs.
    let mut nav = vec![NavLink {
        label: "Dashboard".into(),
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
    // A screen that asked for a menu entry sits beside the resources, before
    // Settings. One without a label mounts silently, reached by a link elsewhere.
    for reg in &app_screens {
        if let Some(label) = &reg.nav_label {
            nav.push(NavLink {
                label: label.clone(),
                path: format!("{admin_path}{}", reg.base_path),
                icon: None,
            });
        }
    }
    nav.push(NavLink {
        label: "Settings".into(),
        path: format!("{admin_path}/settings"),
        icon: Some("settings"),
    });
    resources.extend(app_resources);

    let app_name = if config.app_name.trim().is_empty() {
        "Laterite".to_string()
    } else {
        config.app_name.clone()
    };

    // The picker-source registry, keyed by dotted name. A bad name or a duplicate
    // is a wiring bug, so it aborts boot.
    let mut pickers = picker::PickerRegistry::new();
    for reg in app_picker_sources {
        let name = reg.name.clone();
        if !field::is_name(&name, true) {
            panic!("invalid picker source name `{name}`");
        }
        if pickers.insert(name.clone(), reg).is_some() {
            panic!("duplicate picker source `{name}`");
        }
    }

    // The reference field validates its `source` against the picker registry at
    // boot, so build the field registry here (over the arg-free built-ins) with
    // the picker field added and the registry injected.
    let pickers = Arc::new(pickers);
    let mut field_types = field::builtin_registry();
    field_types.insert(
        "reference".to_string(),
        Arc::new(field::RefPickerField::new(pickers.clone())),
    );

    // The form-persister registry: a bad or duplicate name is a wiring bug, so it
    // aborts boot. It is not on AdminState; `prepare` resolves it per form.
    let mut persisters = persist::PersisterRegistry::new();
    for reg in app_persisters {
        let name = reg.name.clone();
        if !field::is_name(&name, true) {
            panic!("invalid persister name `{name}`");
        }
        if persisters.insert(name.clone(), reg.persister).is_some() {
            panic!("duplicate persister `{name}`");
        }
    }

    // Module-contributed field types join the registry built above, rather than a
    // fresh one: rebuilding here would drop the reference field that was just
    // inserted, since that one needs the picker registry and cannot come from the
    // arg-free built-ins. A module can offer an input the framework has none of,
    // which is what keeps a custom input from meaning a framework change.
    for reg in app_field_types {
        let name = reg.name().to_string();
        if !field::is_name(&name, true) {
            panic!("invalid field type name `{name}`");
        }
        if field_types.insert(name.clone(), reg.field_type).is_some() {
            panic!("duplicate field type `{name}`");
        }
    }

    // The icon set: the framework's curated names, plus whatever modules
    // contributed. A bad name or unusable SVG aborts the boot rather than
    // rendering something wrong later.
    let mut icon_set = laterite_core::icons::IconSet::new();
    for (owner, reg) in app_icons {
        if let Err(e) = icon_set.add(owner.as_str(), &reg) {
            panic!("module `{owner}` contributed an invalid icon: {e}");
        }
    }

    // The sprite joins the asset registry, so it gets a content-named URL and
    // may be cached forever: a changed set is a changed URL.
    let mut assets = builtin_assets();
    assets.insert(
        ICON_SPRITE,
        AdminAsset {
            mime: "image/svg+xml",
            bytes: std::borrow::Cow::Owned(icon_set.sprite().into_bytes()),
        },
    );

    // Every descriptor icon is checked now, while the names are all in hand and
    // nobody is waiting on a response. A typo here used to reach production as a
    // wrong picture that looked like somebody's choice.
    for item in &settings {
        icons::require(
            &icon_set,
            &format!("settings item `{}`", item.code),
            item.icon.as_deref(),
        );
    }
    for link in &nav {
        icons::require(&icon_set, &format!("menu entry `{}`", link.path), link.icon);
    }

    // The column-type registry: the built-ins, plus whatever the modules
    // contributed. A bad or duplicate key is a wiring bug, so it aborts boot the
    // way a duplicate persister or picker source does.
    let mut column_types = list::builtin_column_registry();
    for reg in app_column_types {
        let name = reg.name().to_string();
        if !field::is_name(&name, true) {
            panic!("invalid column type name `{name}`");
        }
        if column_types.insert(name.clone(), reg.column_type).is_some() {
            panic!("duplicate column type `{name}`");
        }
    }

    let settings = Arc::new(settings);
    // One render of the asset URLs, shared by the state and the sprite lookup.
    let asset_urls_arc = Arc::new(asset_urls(&assets));
    let sprite_url: Arc<str> = Arc::from(
        asset_urls_arc
            .get(ICON_SPRITE)
            .map(|p| format!("{admin_path}/assets/{p}"))
            .unwrap_or_default()
            .as_str(),
    );
    let icons_arc = Arc::new(icon_set);
    let state = AdminState {
        auth,
        db,
        nav: Arc::new(nav),
        settings: settings.clone(),
        permissions: Arc::new(permissions),
        admin_path: Arc::from(admin_path.as_str()),
        secure_cookie: config.secure_cookie,
        trusted_proxies: Arc::new(clientip::parse_trusted(&config.trusted_proxies)),
        icons: icons_arc.clone(),
        sprite_url: sprite_url.clone(),
        origin: Arc::from(config.origin.trim_end_matches('/')),
        timezone: config.timezone.parse().unwrap_or(Tz::UTC),
        default_locale: Arc::from(default_locale(&config.locale, &catalogs.locales())),
        catalogs,
        app_name,
        brand_cache: Arc::new(RwLock::new(None)),
        field_types: Arc::new(field_types),
        column_types: Arc::new(column_types),
        plugin_defined,
        assets: Arc::new(assets),
        asset_urls: asset_urls_arc.clone(),
        pickers,
        overrides: Arc::new(field::NoOverrides),
    };

    let mut protected = Router::new().route(&admin_path, get(dashboard));
    for resource in &resources {
        protected = protected.merge(mount_resource(
            resource,
            &state.field_types,
            &state.column_types,
            &permission_codes,
            &persisters,
            &app_listeners,
        ));
    }
    // A module's own screens: routes it wrote, mounted inside the protected tree
    // so they inherit the session, the permission guard, CSRF and the error pages.
    // Nested as a service, so the screen's router carries no framework state.
    for reg in &app_screens {
        let base = format!("{admin_path}{}", reg.base_path);
        let ctx = routes::RouteCtx::new(
            state.db.clone(),
            &base,
            &admin_path,
            &state.origin,
            state.plugin_defined.clone(),
        );
        protected = protected.merge(guard_with_permission(
            Router::new().nest_service(&base, reg.screen.mount(&ctx)),
            &reg.permission,
        ));
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
        Router::new()
            .route(
                &format!("{admin_path}/users/{{id}}/edit"),
                get(users::edit_form).post(users::update),
            )
            .route(
                &format!("{admin_path}/users/{{id}}/active"),
                post(users::set_active),
            ),
        "backend.manage_users",
    ));
    // The plugins screen lists the installed plugins and toggles each on or off
    // (an intent applied on the next boot), gated by its own permission.
    protected = protected.merge(guard_with_permission(
        Router::new()
            .route(&format!("{admin_path}/plugins"), get(plugins::index))
            .route(
                &format!("{admin_path}/plugins/toggle"),
                post(plugins::toggle),
            ),
        plugins::MANAGE_PERMISSION,
    ));
    protected = protected
        .route(&format!("{admin_path}/settings"), get(settings_index))
        .route(
            &format!("{admin_path}/preferences"),
            get(preferences_form).post(preferences_update),
        )
        .route(
            &format!("{admin_path}/preferences/sessions/revoke"),
            post(session_revoke),
        )
        .route(
            &format!("{admin_path}/preferences/sessions/revoke-others"),
            post(session_revoke_others),
        )
        // Picker endpoints, served with the QUERY method (a read; the guard in the
        // handler answers 405 for any other method).
        .route(
            &format!("{admin_path}/pickers/{{source}}/search"),
            any(picker::search),
        )
        .route(
            &format!("{admin_path}/pickers/{{source}}/resolve"),
            any(picker::resolve),
        )
        .route(&format!("{admin_path}/logout"), post(logout));

    // A route per settings screen, at the path its module resolved to, rather
    // than one parameterised route keyed by storage code. The code is captured
    // here, so it never has to survive a trip through the URL.
    for item in settings.iter().filter(|i| i.link.is_none()) {
        let path = format!("{admin_path}{}", item.route());
        let code = item.code.clone();
        let edit_code = code.clone();
        protected = protected.route(
            &path,
            get(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Extension(user): Extension<AuthenticatedUser>| {
                    let code = edit_code.clone();
                    async move { settings_edit(state, shell, user, code).await }
                },
            )
            .post(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Extension(user): Extension<AuthenticatedUser>,
                      Extension(session): Extension<session::SessionHandle>,
                      Form(data): Form<HashMap<String, String>>| {
                    let code = code.clone();
                    async move { settings_update(state, shell, user, session, code, data).await }
                },
            ),
        );
    }

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
        .route(&format!("{admin_path}/assets/{{*path}}"), get(serve_asset))
        // A module's own public endpoints, at the literal paths they declared and
        // outside the admin entirely: no session, no permission, no chrome. They
        // sit above the fallback, so an unmatched URL still renders the 404.
        .merge(mount_public(
            &app_public,
            &state.db,
            &admin_path,
            &state.origin,
            &state.plugin_defined,
        ))
        // Unmatched URLs render the styled 404; a handler panic renders the 500.
        .fallback(not_found_fallback)
        // The CSRF origin gate wraps every route (login and setup included); the
        // panic layer is outermost so it catches everything.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            enforce_origin,
        ))
        // Outside the origin gate, so a rejected request is still logged with
        // the address it came from.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            capture_client,
        ))
        // Admin responses are per-operator and carry a request token, so no
        // shared cache may hold one and no browser may leave one on disk for
        // the next person at the machine. `if_not_present` leaves the asset
        // routes alone: those set their own policy from their URL.
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static(http_cache::PRIVATE_NO_STORE),
            ),
        )
        // An htmx fragment and the full page share a URL and differ only by the
        // request header, so any cache keying on URL alone would serve one for
        // the other.
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                header::VARY,
                axum::http::HeaderValue::from_static("HX-Request"),
            ),
        )
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(
            handle_panic,
        ))
        .with_state(state)
}

/// Fallback for unmatched admin URLs: a styled 404.
async fn not_found_fallback() -> AdminError {
    AdminError::NotFound
}

/// Turns a handler panic into a logged, styled 500.
fn handle_panic(err: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let cause = err
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| err.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string());
    tracing::error!(panic = cause, "admin handler panicked");
    error::masked_500()
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
/// Every contributed public route, nested at its declared path.
fn mount_public(
    regs: &[routes::PublicRouteReg],
    db: &Db,
    admin_path: &str,
    origin: &str,
    plugin_defined: &Arc<laterite_core::Registry>,
) -> Router<AdminState> {
    let mut router = Router::new();
    for reg in regs {
        let ctx = routes::RouteCtx::new(
            db.clone(),
            &reg.path,
            admin_path,
            origin,
            plugin_defined.clone(),
        );
        // A nested service does not inherit the outer fallback, so without this a
        // request *below* a public route (`/robots.txt/anything`) escapes into
        // axum's bodyless 404 instead of the deployment's styled error page.
        // Pinned by a test, because it is a behaviour rather than a guarantee.
        router = router.nest_service(
            &reg.path,
            reg.route.mount(&ctx).fallback(not_found_fallback),
        );
    }
    router
}

/// Moves a resource to `base`, taking its list and form links with it. Used when
/// a module's screens resolve to a namespaced or overridden path.
pub(crate) fn rebase_resource(base: &str, resource: &mut Resource) {
    let old = resource.base_path.clone();
    resource.base_path = base.to_string();
    if let Some(edit_base) = &mut resource.list.edit_base {
        if edit_base == &old {
            *edit_base = base.to_string();
        }
    }
    if let Some(form) = &mut resource.form {
        if form.base_path == old {
            form.base_path = base.to_string();
        }
    }
}

fn prefix_resource(admin_path: &str, resource: &mut Resource) {
    resource.base_path = format!("{admin_path}{}", resource.base_path);
    if let Some(edit_base) = &mut resource.list.edit_base {
        *edit_base = format!("{admin_path}{edit_base}");
    }
    if let Some(form) = &mut resource.form {
        form.base_path = format!("{admin_path}{}", form.base_path);
    }
}

/// One embedded admin asset served by [`serve_asset`].
pub(crate) struct AdminAsset {
    pub mime: &'static str,
    /// Borrowed for assets compiled in, owned for the icon sprite, which is
    /// assembled at boot from the framework's set plus whatever modules
    /// contributed and so cannot come from a file.
    pub bytes: std::borrow::Cow<'static, [u8]>,
}

/// Admin assets served under `{admin}/assets/`, keyed by path. An open registry:
/// field types and plugins contribute their own (wired when the first one does).
pub(crate) type AssetRegistry = HashMap<&'static str, AdminAsset>;

/// Registry key to the path it is served at, each carrying a digest of its
/// bytes. Built once at boot, since the bytes are compiled in and cannot change
/// while the process runs.
pub(crate) type AssetUrls = HashMap<&'static str, String>;

/// Names every registered asset by its content.
pub(crate) fn asset_urls(registry: &AssetRegistry) -> AssetUrls {
    registry
        .iter()
        .map(|(&key, asset)| {
            (
                key,
                http_cache::fingerprint(key, &http_cache::digest(&asset.bytes)),
            )
        })
        .collect()
}

/// Resolves widget asset keys to per-page assets for the shell: an order-
/// preserving dedup, a single URL builder (`{base}/assets/{key}`), and
/// stylesheet-vs-script by the registry's mime. A key absent from the registry is
/// skipped and trips a debug assertion, since a declared asset must be registered.
/// The URL is stamped as `data-lat-asset` in the head so a later htmx fragment's
/// `lat.assets.ensure` of the same key is a no-op.
pub(crate) fn page_assets(
    keys: &[&str],
    base: &str,
    registry: &AssetRegistry,
    urls: &AssetUrls,
) -> Vec<PageAsset> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for &key in keys {
        if !seen.insert(key) {
            continue;
        }
        match (registry.get(key), urls.get(key)) {
            (Some(asset), Some(path)) => out.push(PageAsset {
                url: format!("{base}/assets/{path}"),
                css: asset.mime.contains("css"),
            }),
            _ => debug_assert!(false, "asset key `{key}` is not registered"),
        }
    }
    out
}

/// The framework's built-in assets: the stylesheet, brand marks, and webfonts.
///
/// None of them declares a cache policy. Every one is referenced through
/// [`asset_urls`], which names each by a digest of its bytes, and
/// [`serve_asset`] reads the policy off the URL: a request that named the
/// content may keep it forever, one that did not must revalidate. So an asset
/// changed by a framework upgrade reaches a browser that had cached the old
/// one, without anybody remembering to say so.
pub(crate) fn builtin_assets() -> AssetRegistry {
    let font = |bytes: &'static [u8]| AdminAsset {
        mime: "font/woff2",
        bytes: std::borrow::Cow::Borrowed(bytes),
    };
    HashMap::from([
        (
            "laterite.css",
            AdminAsset {
                mime: "text/css; charset=utf-8",
                bytes: std::borrow::Cow::Borrowed(include_bytes!("../assets/laterite.css")),
            },
        ),
        (
            "laterite.js",
            AdminAsset {
                mime: "text/javascript; charset=utf-8",
                bytes: std::borrow::Cow::Borrowed(include_bytes!("../assets/laterite.js")),
            },
        ),
        (
            "vendor/htmx.min.js",
            AdminAsset {
                mime: "text/javascript; charset=utf-8",
                bytes: std::borrow::Cow::Borrowed(include_bytes!("../assets/vendor/htmx.min.js")),
            },
        ),
        (
            "fields/ref-picker.js",
            AdminAsset {
                mime: "text/javascript; charset=utf-8",
                bytes: std::borrow::Cow::Borrowed(include_bytes!("../assets/fields/ref-picker.js")),
            },
        ),
        (
            "fields/ref-picker.css",
            AdminAsset {
                mime: "text/css; charset=utf-8",
                bytes: std::borrow::Cow::Borrowed(include_bytes!(
                    "../assets/fields/ref-picker.css"
                )),
            },
        ),
        (
            "mark.svg",
            AdminAsset {
                mime: "image/svg+xml",
                bytes: std::borrow::Cow::Borrowed(include_bytes!("../assets/mark.svg")),
            },
        ),
        (
            "mark.png",
            AdminAsset {
                mime: "image/png",
                bytes: std::borrow::Cow::Borrowed(include_bytes!("../assets/mark.png")),
            },
        ),
        (
            "fonts/space-grotesk-500.woff2",
            font(include_bytes!("../assets/fonts/space-grotesk-500.woff2")),
        ),
        (
            "fonts/space-grotesk-600.woff2",
            font(include_bytes!("../assets/fonts/space-grotesk-600.woff2")),
        ),
        (
            "fonts/space-grotesk-700.woff2",
            font(include_bytes!("../assets/fonts/space-grotesk-700.woff2")),
        ),
        (
            "fonts/ibm-plex-sans-400.woff2",
            font(include_bytes!("../assets/fonts/ibm-plex-sans-400.woff2")),
        ),
        (
            "fonts/ibm-plex-sans-600.woff2",
            font(include_bytes!("../assets/fonts/ibm-plex-sans-600.woff2")),
        ),
        (
            "fonts/ibm-plex-mono-400.woff2",
            font(include_bytes!("../assets/fonts/ibm-plex-mono-400.woff2")),
        ),
        (
            "fonts/ibm-plex-mono-600.woff2",
            font(include_bytes!("../assets/fonts/ibm-plex-mono-600.woff2")),
        ),
    ])
}

/// Serves an embedded admin asset by path (public; no auth).
async fn serve_asset(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Response {
    // A URL that named the content is answerable forever, because different
    // bytes would have been a different URL. One that did not gets a validator
    // instead, so the next request is a cheap 304 rather than a fresh download.
    let (key, fingerprinted) = http_cache::strip_fingerprint(&path);
    let Some(asset) = state.assets.get(key.as_str()) else {
        return not_found();
    };
    let policy = if fingerprinted {
        http_cache::IMMUTABLE
    } else {
        http_cache::REVALIDATE
    };
    http_cache::conditional(&headers, asset.mime, policy, asset.bytes.to_vec())
}

/// Builds a resource's list, create, and edit routes as generic handlers that
/// carry the resource's descriptors. When the resource sets a `permission`, every
/// route it mounts is wrapped in a guard that answers `403 Forbidden` for an
/// operator who lacks it; the caller merges the result into the protected router.
fn mount_resource(
    resource: &Resource,
    field_types: &field::FieldRegistry,
    column_types: &list::ColumnRegistry,
    permission_codes: &std::collections::HashSet<String>,
    persisters: &persist::PersisterRegistry,
    listeners: &[laterite_core::ModelListenerReg],
) -> Router<AdminState> {
    // A column naming a type nobody registered used to render an empty cell;
    // it aborts boot instead, like a form field with an unregistered type. So
    // does a column requiring a permission nobody registered.
    list::check(&resource.list, column_types, permission_codes)
        .unwrap_or_else(|e| panic!("admin resource `{}`: {e}", resource.base_path));
    // A write resource must be permission-gated. A create/edit form with no
    // permission would expose its mutations to every signed-in operator, so an
    // unguarded form is a wiring bug that aborts boot (a read-only list may omit a
    // permission). If a public write resource is ever wanted, it opts in explicitly.
    if resource.form.is_some() && resource.permission.is_none() {
        panic!(
            "admin resource `{}` has a create/edit form but no permission; a write \
             resource must declare a permission",
            resource.base_path
        );
    }
    let base = resource.base_path.clone();
    let list_path = base.clone();
    let list_cfg = resource.list.clone();
    let mut router = Router::new().route(
        &base,
        get(
            move |State(state): State<AdminState>,
                  Extension(shell): Extension<Shell>,
                  Query(params): Query<list::ListParams>,
                  Query(raw): Query<std::collections::HashMap<String, String>>,
                  Extension(user): Extension<AuthenticatedUser>,
                  headers: axum::http::HeaderMap| {
                let cfg = list_cfg.clone();
                let path = list_path.clone();
                async move {
                    list::handle(&state, &cfg, &path, params, &raw, &user, shell, &headers).await
                }
            },
        ),
    );

    // Not registered at all when the resource has not opted in, so the capability
    // cannot be reached by guessing the URL.
    let (export_cfg, export_path) = (resource.list.clone(), resource.base_path.clone());
    if resource.list.exportable {
        router = router.route(
            &format!("{base}/export"),
            get(
                move |state: State<AdminState>,
                      Query(raw): Query<std::collections::HashMap<String, String>>,
                      Extension(user): Extension<AuthenticatedUser>,
                      Extension(shell): Extension<Shell>,
                      Extension(session): Extension<session::SessionHandle>| {
                    let cfg = export_cfg.clone();
                    let path = export_path.clone();
                    async move {
                        export::handle(state, &cfg, &path, &raw, &user, &shell, &session).await
                    }
                },
            ),
        );
    }

    let (columns_cfg, columns_path) = (resource.list.clone(), resource.base_path.clone());
    router = router.route(
        &format!("{base}/columns"),
        post(
            move |State(state): State<AdminState>,
                  Extension(user): Extension<AuthenticatedUser>,
                  Extension(session): Extension<session::SessionHandle>,
                  headers: axum::http::HeaderMap,
                  Form(pairs): Form<Vec<(String, String)>>| {
                let cfg = columns_cfg.clone();
                let path = columns_path.clone();
                async move {
                    list::set_columns(&state, &cfg, &path, &user, &session, &headers, &pairs).await
                }
            },
        ),
    );

    if resource.list.deletable {
        // The write path for a delete: the form's persister when the resource has
        // a generic form, otherwise one over the list's own table, so a bespoke
        // editor (roles) still deletes through the pipeline rather than beside it.
        let persister: Arc<dyn persist::Persister> =
            match resource.form.as_ref().and_then(|f| f.persist.as_ref()) {
                Some(key) => persisters.get(key).cloned().unwrap_or_else(|| {
                    panic!(
                        "admin resource `{}`: unregistered persister `{key}`",
                        resource.base_path
                    )
                }),
                None => Arc::new(persist::DefaultPersister::from_list(&resource.list)),
            };
        let ctx = Arc::new(bulk::BulkContext {
            base_path: resource.base_path.clone(),
            entity: resource.list.entity.clone(),
            persister,
            listeners: listeners
                .iter()
                .filter(|reg| reg.matches(&resource.list.entity))
                .map(|reg| reg.listener.clone())
                .collect(),
        });
        router =
            router.route(
                &format!("{base}/delete"),
                post(
                    move |state: State<AdminState>,
                          user: Extension<AuthenticatedUser>,
                          session: Extension<session::SessionHandle>,
                          headers: axum::http::HeaderMap,
                          form: Form<Vec<(String, String)>>| {
                        let ctx = ctx.clone();
                        async move {
                            bulk::delete_handler(state, user, session, ctx, headers, form).await
                        }
                    },
                ),
            );
    }

    if let Some(form_cfg) = resource.form.clone() {
        // Resolve the form's field options once, here at router build. A malformed
        // option or unregistered type aborts boot naming the resource.
        let prepared = Arc::new(
            form::PreparedForm::prepare(form_cfg, field_types, persisters, listeners)
                .unwrap_or_else(|e| panic!("admin resource `{}`: {e}", resource.base_path)),
        );
        let (new_pf, create_pf) = (prepared.clone(), prepared.clone());
        router = router.route(
            &format!("{base}/new"),
            get(
                move |State(state): State<AdminState>, Extension(shell): Extension<Shell>| {
                    let pf = new_pf.clone();
                    async move { form::new_form(&state, &pf, shell) }
                },
            )
            .post(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Extension(user): Extension<AuthenticatedUser>,
                      Extension(session): Extension<session::SessionHandle>,
                      headers: axum::http::HeaderMap,
                      Form(data): Form<HashMap<String, String>>| {
                    let pf = create_pf.clone();
                    async move {
                        form::create(&state, &pf, data, shell, &user, &session, &headers).await
                    }
                },
            ),
        );

        let (edit_pf, update_pf) = (prepared.clone(), prepared.clone());
        router = router.route(
            &format!("{base}/{{id}}/edit"),
            get(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Path(id): Path<String>| {
                    let pf = edit_pf.clone();
                    async move { form::edit_form(&state, &pf, id, shell).await }
                },
            )
            .post(
                move |State(state): State<AdminState>,
                      Extension(shell): Extension<Shell>,
                      Extension(user): Extension<AuthenticatedUser>,
                      Extension(session): Extension<session::SessionHandle>,
                      Path(id): Path<String>,
                      headers: axum::http::HeaderMap,
                      Form(data): Form<HashMap<String, String>>| {
                    let pf = update_pf.clone();
                    async move {
                        form::update(&state, &pf, id, data, shell, &user, &session, &headers).await
                    }
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

/// The gate for authenticated admin routes. Resolves the session (identity plus
/// its opaque blob), verifies CSRF on state-changing requests, injects the
/// identity, shell, and a [`session::SessionHandle`] for handlers, and persists
/// the session blob afterwards when a handler changed it. Unauthenticated
/// requests redirect to the login screen.
async fn require_auth(
    State(state): State<AdminState>,
    jar: CookieJar,
    request: Request,
    next: Next,
) -> Response {
    let login = format!("{}/login", state.admin_path);
    let client_ctx = request
        .extensions()
        .get::<RequestContext>()
        .cloned()
        .unwrap_or_default();
    // A live session is the ordinary path. A missing or expired one falls back
    // to the stay-signed-in credential, which mints a fresh session and rotates
    // itself, so the cookie just presented is never accepted a second time.
    let live = match jar.get(SESSION_COOKIE) {
        Some(cookie) => {
            let token = cookie.value().to_string();
            state
                .auth
                .resolve_session(&token)
                .await
                .ok()
                .map(|resolved| (token, resolved))
        }
        None => None,
    };
    let (token, resolved, recalled) = match live {
        Some((token, resolved)) => (token, resolved, None),
        None => match recall(&state, &jar, &client_ctx).await {
            Some((token, resolved, credential)) => (token, resolved, Some(credential)),
            // Clear the credential on the way out: a cookie that failed once
            // will fail every time, and retrying it on each request is noise.
            None => {
                return (
                    jar.remove(remember_removal(&state.admin_path)),
                    Redirect::to(&login),
                )
                    .into_response()
            }
        },
    };
    let handle = session::SessionHandle::from_blob(resolved.data.as_deref());

    // CSRF token check (the origin gate already ran on the whole router). A
    // state-changing method must carry the session token in a header or the
    // `_csrf` field; buffering the body serves plain forms and HTMX alike and
    // fails closed when a form omitted the field.
    let mut request = request;
    let safe = session::is_safe_method(request.method());
    // Set when the token has to be checked by the handler's extractor instead.
    let mut deferred: Option<session::CsrfPending> = None;
    if !safe {
        let (parts, body) = request.into_parts();
        // A file upload is never buffered: the form limit would truncate it to
        // nothing (and the token check would then fail for the wrong reason),
        // and holding a whole file in memory to read one field defeats streaming
        // it. Such a request carries its token in the header, or in the form
        // action's query string when there is no scripting to set one. The body
        // passes through untouched.
        let (body, submitted) = if session::is_multipart(&parts.headers) {
            let token = session::header_token(&parts.headers)
                .or_else(|| session::query_token(parts.uri.query()));
            // No token beside the body means it is in the body, where the
            // reference system reads it from and where a plain form can put it.
            // The guard cannot look there without consuming the upload, so the
            // check is deferred to the extractor and enforced below.
            if token.is_none() {
                deferred = Some(session::CsrfPending::default());
            }
            (body, token)
        } else {
            let bytes = axum::body::to_bytes(body, MAX_FORM_BYTES)
                .await
                .unwrap_or_default();
            let token = session::submitted_token(&parts.headers, &bytes);
            (Body::from(bytes), token)
        };
        if deferred.is_none() && !session::token_matches(&handle.csrf_token(), submitted.as_deref())
        {
            tracing::warn!(
                user_id = resolved.identity.user.id,
                "admin CSRF check failed"
            );
            return error::csrf_rejected();
        }
        request = Request::from_parts(parts, body);
    }

    // Deliver and clear any queued flash on a full-page render; a mutating
    // request leaves it for the redirect target's GET.
    let flash = if safe {
        handle.take_flash()
    } else {
        Vec::new()
    };
    let user = resolved.identity;
    // Kept past the move into the request, for the deferred-check log below.
    let acting_user_id = user.user.id;
    let path = request.uri().path().to_string();
    // Resolve the locale chain for this request (operator preference, then the
    // browser's Accept-Language, then the deployment default) over the shared
    // catalogs, before the nav context, which localizes the settings sidebar.
    let accept_language = request
        .headers()
        .get("accept-language")
        .and_then(|v| v.to_str().ok());
    let serveable = state.catalogs.locales();
    let chain = resolve_locale_chain(
        user.user.locale.as_deref(),
        accept_language,
        &state.default_locale,
        &serveable,
    );
    let i18n = Translator::with_chain(chain, state.catalogs.clone());
    // A clone kept past `next` (the shell's copy moves into the request) to localize
    // an error response the handler renders English (the `ErrorMeta` seam).
    let translator = i18n.clone();
    let (sidebar, active_nav) = resolve_nav_context(
        &state.nav,
        &state.settings,
        &state.admin_path,
        &user.permissions,
        &path,
        &i18n,
        icons::Icons::new(&state.icons, &state.sprite_url),
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
        handle.csrf_token(),
        flash,
        i18n,
        state.asset_urls.clone(),
        state.origin.to_string(),
        state.icons.clone(),
        state.sprite_url.clone(),
    );
    request.extensions_mut().insert(user);
    request.extensions_mut().insert(shell);
    request.extensions_mut().insert(handle.clone());
    // Present only when the token check was deferred to an upload extractor.
    if let Some(pending) = &deferred {
        request.extensions_mut().insert(pending.clone());
    }
    let mut response = next.run(request).await;

    // A deferred check that never happened means the handler took the upload
    // without the verifying extractor. Refuse the response rather than let the
    // route quietly run unguarded: the origin gate held, but that is the outer
    // layer, not the whole of it.
    if let Some(pending) = &deferred {
        if !pending.was_satisfied() {
            tracing::error!(
                user_id = acting_user_id,
                "a multipart route returned without verifying its request token"
            );
            return error::csrf_rejected();
        }
    }

    // The `ErrorMeta` seam: an `AdminError` renders its page in English from
    // `IntoResponse` (no request context there); re-render it localized now that a
    // translator is in hand. Status and kind are preserved.
    if let Some(meta) = response.extensions().get::<error::ErrorMeta>().copied() {
        response = error::localized_error(meta.kind, &translator);
    }

    // Persist the blob only when a handler changed it (flash set or consumed,
    // token rotated, or a token freshly minted this request).
    if let Some(blob) = handle.dirty_blob() {
        if let Err(e) = state.auth.set_session_data(&token, &blob).await {
            tracing::error!(error = %e, "persisting admin session failed");
        }
    }
    // A recall replaced both halves: the session it minted and the credential
    // that replaced the one just spent.
    if let Some(credential) = recalled {
        let jar = jar
            .add(session_cookie(
                token,
                &state.admin_path,
                state.secure_cookie,
            ))
            .add(remember_cookie(
                credential,
                &state.admin_path,
                state.secure_cookie,
            ));
        return (jar, response).into_response();
    }
    response
}

/// Trades a presented stay-signed-in cookie for a live session. Returns the new
/// session token, the resolved identity, and the credential that replaces the
/// one just spent.
async fn recall(
    state: &AdminState,
    jar: &CookieJar,
    ctx: &RequestContext,
) -> Option<(
    String,
    laterite_auth::ResolvedSession,
    laterite_auth::RememberCredential,
)> {
    let presented = jar.get(REMEMBER_COOKIE)?.value().to_string();
    let recalled = state.auth.consume_remember(&presented, ctx).await.ok()?;
    let token = recalled.session.token.clone();
    let resolved = state.auth.resolve_session(&token).await.ok()?;
    Some((token, resolved, recalled.remember))
}

/// Ceiling on a buffered admin form body for the CSRF check. Admin forms are
/// small; a larger body is rejected rather than buffered.
const MAX_FORM_BYTES: usize = 1024 * 1024;

/// The primary CSRF gate, layered on the whole admin router: a state-changing
/// request must come from our own origin (see [`session::origin_ok`]). This
/// covers the login and setup screens, which are deliberately token-less (no
/// session exists yet), so the origin check is their sole CSRF defense.
/// Records where a request came from, once, for anything downstream that logs.
/// Doing it in one layer keeps the trusted-proxy rule in a single place rather
/// than at each call site that happens to want an address.
async fn capture_client(
    State(state): State<AdminState>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0);
    let headers = request.headers();
    let ctx = RequestContext {
        ip_address: clientip::client_ip(peer, headers, &state.trusted_proxies),
        user_agent: clientip::user_agent(headers),
    };
    request.extensions_mut().insert(ctx);
    next.run(request).await
}

async fn enforce_origin(State(state): State<AdminState>, request: Request, next: Next) -> Response {
    if !session::is_safe_method(request.method())
        && !session::origin_ok(request.headers(), &state.origin)
    {
        tracing::warn!(
            path = request.uri().path(),
            "admin CSRF origin check failed"
        );
        return error::csrf_rejected();
    }
    next.run(request).await
}

async fn login_form(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    // A fresh install with no operators goes to first-run setup instead.
    match state.auth.has_any_operator().await {
        Ok(false) => Redirect::to(&format!("{}/setup", state.admin_path)).into_response(),
        Ok(true) => render(LoginTemplate {
            base: state.admin_path.to_string(),
            asset_urls: state.asset_urls.clone(),
            brand: state.brand().await,
            error: None,
            i18n: pre_auth_translator(&state, &headers),
        }),
        Err(_) => render_error(),
    }
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    /// The "Stay signed in" box. Present only when ticked, as a checkbox is.
    #[serde(default)]
    remember: Option<String>,
}

async fn login_submit(
    State(state): State<AdminState>,
    jar: CookieJar,
    headers: HeaderMap,
    Extension(ctx): Extension<RequestContext>,
    Form(form): Form<LoginForm>,
) -> Response {
    match state
        .auth
        .authenticate(&form.username, &form.password, &ctx)
        .await
    {
        Ok(session) => {
            let wants_remember = form.remember.is_some();
            let user_id = session.user_id;
            let mut jar = jar.add(session_cookie(
                session.token,
                &state.admin_path,
                state.secure_cookie,
            ));
            if wants_remember {
                match state.auth.issue_remember(user_id).await {
                    Ok(credential) => {
                        jar = jar.add(remember_cookie(
                            credential,
                            &state.admin_path,
                            state.secure_cookie,
                        ));
                    }
                    // The session still stands; the box just did not take.
                    Err(e) => {
                        tracing::error!(error = %e, "issuing a stay-signed-in credential failed")
                    }
                }
            }
            (jar, Redirect::to(&state.admin_path)).into_response()
        }
        Err(_) => {
            let i18n = pre_auth_translator(&state, &headers);
            render(LoginTemplate {
                base: state.admin_path.to_string(),
                asset_urls: state.asset_urls.clone(),
                brand: state.brand().await,
                error: Some(i18n.t(&t!("Invalid username or password."))),
                i18n,
            })
        }
    }
}

/// Builds the session cookie, scoped to the admin mount and flagged `Secure`
/// behind HTTPS. Shared by login and first-run setup.
///
/// `remember` is what the login form's "Stay signed in" box asks for: the cookie
/// is given the session's own lifetime, so closing the browser does not end it.
/// Without it the cookie lasts the browser session, which is the right default
/// for a shared machine. Either way the cookie never outlives the session row it
/// names, because both take their length from the same place.
fn session_cookie(token: String, admin_path: &str, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .path(admin_path.to_string())
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

/// The "stay signed in" cookie. Outlives the session deliberately: when the
/// session ends, this is what mints the next one.
fn remember_cookie(
    credential: laterite_auth::RememberCredential,
    admin_path: &str,
    secure: bool,
) -> Cookie<'static> {
    let seconds = (credential.expires_at - chrono::Utc::now())
        .num_seconds()
        .max(0);
    Cookie::build((REMEMBER_COOKIE, credential.cookie))
        .path(admin_path.to_string())
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(seconds))
        .build()
}

/// Clears the remember cookie, for a logout or a credential that failed.
fn remember_removal(admin_path: &str) -> Cookie<'static> {
    Cookie::build((REMEMBER_COOKIE, ""))
        .path(admin_path.to_string())
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
async fn setup_form(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    match state.auth.has_any_operator().await {
        Ok(true) => Redirect::to(&format!("{}/login", state.admin_path)).into_response(),
        Ok(false) => render(setup_view(
            &state.admin_path,
            state.brand().await,
            state.timezone,
            None,
            pre_auth_translator(&state, &headers),
            state.asset_urls.clone(),
        )),
        Err(_) => render_error(),
    }
}

async fn setup_submit(
    State(state): State<AdminState>,
    jar: CookieJar,
    headers: HeaderMap,
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
            Some(t!(
                "Username, first name, email, and password are all required."
            )),
            pre_auth_translator(&state, &headers),
            state.asset_urls.clone(),
        ));
    }
    // The setup select always carries a value, but guard against a bad one.
    if tz.parse::<Tz>().is_err() {
        return render(setup_view(
            &state.admin_path,
            state.brand().await,
            state.timezone,
            Some(t!("That is not a recognised timezone.")),
            pre_auth_translator(&state, &headers),
            state.asset_urls.clone(),
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
            Some(t!(
                "Could not create the account. The username or email may already be taken."
            )),
            pre_auth_translator(&state, &headers),
            state.asset_urls.clone(),
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
    error: Option<Text>,
    i18n: Translator,
    asset_urls: Arc<AssetUrls>,
) -> SetupTemplate {
    let default_name = default_tz.name();
    let zones = TZ_VARIANTS
        .iter()
        .filter(|tz| zone_offered(tz.name()))
        .map(|tz| TzOption {
            name: tz.name().to_string(),
            selected: tz.name() == default_name,
        })
        .collect();
    SetupTemplate {
        base: admin_path.to_string(),
        asset_urls,
        brand,
        zones,
        error: error.map(|e| i18n.t(&e)),
        i18n,
    }
}

async fn logout(State(state): State<AdminState>, jar: CookieJar) -> Response {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let _ = state.auth.logout(cookie.value()).await;
    }
    // Without this the next request would present the credential and sign
    // straight back in, which is not what pressing Sign out means.
    if let Some(cookie) = jar.get(REMEMBER_COOKIE) {
        let _ = state.auth.revoke_remember(cookie.value()).await;
    }
    let removal = Cookie::build((SESSION_COOKIE, ""))
        .path(state.admin_path.to_string())
        .build();
    (
        jar.remove(removal)
            .remove(remember_removal(&state.admin_path)),
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
    state: AdminState,
    shell: Shell,
    user: AuthenticatedUser,
    code: String,
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
    state: AdminState,
    shell: Shell,
    user: AuthenticatedUser,
    session: session::SessionHandle,
    code: String,
    data: HashMap<String, String>,
) -> Response {
    let items = visible_settings(&state.settings, &user.permissions);
    match items
        .iter()
        .find(|item| item.code == code && item.link.is_none())
    {
        Some(item) => settings::update(&state, item, data, shell, &session, &user).await,
        None => not_found(),
    }
}

/// The self-service Preferences screen for the signed-in operator.
async fn preferences_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    jar: CookieJar,
) -> Response {
    let sessions = session_rows(&state, &user, &shell, &jar).await;
    render(preferences_view(
        &shell,
        &user,
        state.timezone,
        &state.default_locale,
        &offered_locales(&state.catalogs),
        sessions,
        None,
    ))
}

#[derive(Deserialize)]
struct RevokeSessionForm {
    id: String,
}

/// Ends one of the account's other sessions.
async fn session_revoke(
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<session::SessionHandle>,
    Form(form): Form<RevokeSessionForm>,
) -> Response {
    match state.auth.revoke_session(user.user.id, &form.id).await {
        Ok(true) => session.push_flash(FlashLevel::Success, t!("That session was signed out.")),
        Ok(false) => session.push_flash(FlashLevel::Info, t!("That session had already ended.")),
        Err(e) => {
            tracing::error!(error = %e, "revoking a session failed");
            session.push_flash(
                FlashLevel::Error,
                t!("That session could not be signed out."),
            );
        }
    }
    Redirect::to(&format!("{}/preferences", state.admin_path)).into_response()
}

/// Ends every session but this one, and drops every stay-signed-in credential.
async fn session_revoke_others(
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<session::SessionHandle>,
    jar: CookieJar,
) -> Response {
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .unwrap_or_default();
    match state.auth.sign_out_everywhere(user.user.id, &token).await {
        Ok(_) => session.push_flash(
            FlashLevel::Success,
            t!("Every other session was signed out."),
        ),
        Err(e) => {
            tracing::error!(error = %e, "signing out other sessions failed");
            session.push_flash(
                FlashLevel::Error,
                t!("Those sessions could not be signed out."),
            );
        }
    }
    // The credential this browser holds went with the rest, so clear its cookie
    // rather than leave one that will fail on the next request.
    (
        jar.remove(remember_removal(&state.admin_path)),
        Redirect::to(&format!("{}/preferences", state.admin_path)),
    )
        .into_response()
}

#[derive(Deserialize)]
struct PreferencesForm {
    timezone: String,
    /// Omitted means no locale choice was submitted, treated as inherit.
    #[serde(default)]
    locale: String,
}

async fn preferences_update(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<session::SessionHandle>,
    jar: CookieJar,
    Form(form): Form<PreferencesForm>,
) -> Response {
    let offered = offered_locales(&state.catalogs);
    // An empty choice clears a preference so the operator inherits the default.
    let tz = form.timezone.trim();
    let tz_stored = if tz.is_empty() {
        None
    } else if tz.parse::<Tz>().is_ok() {
        Some(tz)
    } else {
        return render(preferences_view(
            &shell,
            &user,
            state.timezone,
            &state.default_locale,
            &offered,
            session_rows(&state, &user, &shell, &jar).await,
            Some(t!("That is not a recognised timezone.")),
        ));
    };
    let loc = form.locale.trim();
    let loc_stored = if loc.is_empty() {
        None
    } else if offered.iter().any(|l| l == loc) {
        Some(loc)
    } else {
        return render(preferences_view(
            &shell,
            &user,
            state.timezone,
            &state.default_locale,
            &offered,
            session_rows(&state, &user, &shell, &jar).await,
            Some(t!("That is not a supported language.")),
        ));
    };
    if state
        .auth
        .set_user_timezone(user.user.id, tz_stored)
        .await
        .is_err()
    {
        return render_error();
    }
    match state.auth.set_user_locale(user.user.id, loc_stored).await {
        Ok(()) => {
            session.push_flash(session::FlashLevel::Success, t!("Preferences saved."));
            Redirect::to(&format!("{}/preferences", state.admin_path)).into_response()
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
    default_locale: &str,
    offered: &[String],
    sessions: Vec<SessionRow>,
    error: Option<Text>,
) -> PreferencesTemplate {
    let current = user.user.timezone.as_deref();
    let zones = TZ_VARIANTS
        .iter()
        .filter(|tz| zone_offered(tz.name()))
        .map(|tz| TzOption {
            name: tz.name().to_string(),
            selected: current == Some(tz.name()),
        })
        .collect();
    let current_locale = user.user.locale.as_deref();
    let locales = offered
        .iter()
        .map(|code| LocaleOption {
            name: locale_name(code),
            selected: current_locale == Some(code.as_str()),
            code: code.clone(),
        })
        .collect();
    PreferencesTemplate {
        shell: shell.clone(),
        zones,
        effective_tz: shell.tz.name().to_string(),
        default_tz: default_tz.name().to_string(),
        inherits: current.is_none(),
        locales,
        default_locale: locale_name(default_locale),
        inherits_locale: current_locale.is_none(),
        sessions,
        error: error.map(|e| shell.tt(&e)),
    }
}

/// Reads the account's live sessions and formats them in the operator's own
/// timezone and locale, the same way list columns render a timestamp.
async fn session_rows(
    state: &AdminState,
    user: &AuthenticatedUser,
    shell: &Shell,
    jar: &CookieJar,
) -> Vec<SessionRow> {
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .unwrap_or_default();
    let locale = list::date_locale(shell.locale());
    let stamp = |dt: chrono::DateTime<chrono::Utc>| {
        list::format_ts(&dt.to_rfc3339(), shell.tz, locale, "%-d %b %Y, %H:%M")
    };
    match state.auth.list_sessions(user.user.id, &token).await {
        Ok(sessions) => sessions
            .into_iter()
            .map(|s| SessionRow {
                id: s.id,
                started: stamp(s.created_at),
                last_seen: stamp(s.last_seen_at),
                expires: stamp(s.expires_at),
                device: clientip::describe(s.user_agent.as_deref())
                    .or(s.user_agent)
                    .unwrap_or_else(|| "-".to_string()),
                ip_address: s.ip_address.unwrap_or_default(),
                current: s.current,
            })
            .collect(),
        Err(e) => {
            tracing::error!(error = %e, "listing sessions failed");
            Vec::new()
        }
    }
}

/// The framework's own admin screens.
fn builtin_resources() -> Vec<Resource> {
    vec![
        Resource {
            base_path: "/users".to_string(),
            nav_label: "Backend Users".into(),
            list: backend_users_list_config(),
            form: None,
            permission: Some("backend.manage_users".to_string()),
        },
        Resource {
            base_path: "/roles".to_string(),
            nav_label: "Roles".into(),
            list: roles_list_config(),
            // The create/edit form is the dedicated permission editor (see the
            // `roles` module), mounted separately, not the generic form.
            form: None,
            permission: Some("backend.manage_roles".to_string()),
        },
        Resource {
            base_path: "/audit-log".to_string(),
            nav_label: "Audit Log".into(),
            list: audit_log_list_config(),
            // Append-only: read-only list, no form, no edit or create links.
            form: None,
            permission: Some("backend.view_audit_log".to_string()),
        },
    ]
}

/// The framework's own settings items. The built-in Users and Roles resources
/// appear in the settings menu under a Users category (linking to their list
/// screens), rather than as main-menu tabs.
fn builtin_settings() -> Vec<settings::SettingsItem> {
    vec![
        settings::SettingsItem::new("backend.administrators", "Administrators", Vec::new())
            .description("Manage backend administrator accounts.")
            .category("Users")
            .order(10)
            .icon("users")
            .permission("backend.manage_users")
            .link("/users"),
        settings::SettingsItem::new("backend.roles", "Roles", Vec::new())
            .description("Manage roles and their permissions.")
            .category("Users")
            .order(20)
            .icon("shield")
            .permission("backend.manage_roles")
            .link("/roles"),
        settings::brand::settings_item(),
        settings::SettingsItem::new("backend.plugins", "Plugins", Vec::new())
            .description("Enable or disable installed plugins.")
            .category("System")
            .order(10)
            .icon("plug")
            .permission(plugins::MANAGE_PERMISSION)
            .link("/plugins"),
        settings::SettingsItem::new("backend.audit_log", "Audit Log", Vec::new())
            .description("Review the record of administrative changes.")
            .category("System")
            .order(20)
            .icon("history")
            .permission("backend.view_audit_log")
            .link("/audit-log"),
    ]
}

fn backend_users_list_config() -> list::ListConfig {
    list::ListConfig {
        entity: "backend_users".to_string(),
        title: "Backend Users".into(),
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
        // Rows link to the per-user permission editor; users are created from the
        // CLI or first-run setup, so no "New" screen here.
        edit_base: Some("/users".to_string()),
        filters: vec![
            list::ListFilter::boolean("is_active", "Active"),
            list::ListFilter::boolean("is_superuser", "Superuser"),
        ],
        // Operators are created by the CLI and first-run setup, and removing one
        // from a list is too easy to do by accident; deactivating is the reversible
        // equivalent and is what the filter above is for.
        ..Default::default()
    }
}

fn roles_list_config() -> list::ListConfig {
    list::ListConfig {
        entity: "backend_roles".to_string(),
        title: "Roles".into(),
        columns: vec![
            list::ListColumn::new("code", "Code"),
            list::ListColumn::new("name", "Name"),
            list::ListColumn::new("created_at", "Created").datetime(),
        ],
        order_by: "created_at".to_string(),
        edit_base: Some("/roles".to_string()),
        creatable: true,
        deletable: true,
        ..Default::default()
    }
}

/// The read-only audit-log view: the recorded administrative changes, newest
/// first. Append-only, so no create or edit path (`edit_base`/`creatable` off).
fn audit_log_list_config() -> list::ListConfig {
    list::ListConfig {
        entity: "backend_audit_log".to_string(),
        title: "Audit Log".into(),
        columns: vec![
            list::ListColumn::new("created_at", "When").datetime(),
            list::ListColumn::new("actor_username", "Operator"),
            list::ListColumn::new("action", "Action"),
            list::ListColumn::new("target_type", "Target"),
            list::ListColumn::new("target_id", "Target ID"),
        ],
        order_by: "created_at".to_string(),
        per_page: 50,
        per_page_options: vec![25, 50, 100],
        // The log is evidence: it can leave the panel for an auditor, but nothing
        // removes from it here.
        exportable: true,
        ..Default::default()
    }
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    /// The admin mount path, so pre-auth asset and form URLs match the panel.
    base: String,
    /// Each asset's content-named path; see [`Shell::asset`].
    asset_urls: Arc<AssetUrls>,
    brand: String,
    error: Option<String>,
    /// The pre-auth translator (config locale, then `Accept-Language`); no operator
    /// preference exists yet. `self.t` and `self.locale` localize this screen.
    i18n: Translator,
}

/// Builds a content-named asset URL for the pre-auth screens, which render
/// before a [`Shell`] exists and so carry the map themselves.
fn preauth_asset(urls: &AssetUrls, base: &str, key: &str) -> String {
    match urls.get(key) {
        Some(path) => format!("{base}/assets/{path}"),
        None => {
            debug_assert!(false, "asset key `{key}` is not registered");
            format!("{base}/assets/{key}")
        }
    }
}

impl LoginTemplate {
    fn asset(&self, key: &str) -> String {
        preauth_asset(&self.asset_urls, &self.base, key)
    }
    fn t(&self, source: &str) -> String {
        self.i18n.t(&Text::dynamic(source))
    }
    fn locale(&self) -> &str {
        self.i18n.locale()
    }
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
    /// Each asset's content-named path; see [`Shell::asset`].
    asset_urls: Arc<AssetUrls>,
    brand: String,
    zones: Vec<TzOption>,
    error: Option<String>,
    /// The pre-auth translator, as on [`LoginTemplate`].
    i18n: Translator,
}

impl SetupTemplate {
    fn asset(&self, key: &str) -> String {
        preauth_asset(&self.asset_urls, &self.base, key)
    }
    fn t(&self, source: &str) -> String {
        self.i18n.t(&Text::dynamic(source))
    }
    fn locale(&self) -> &str {
        self.i18n.locale()
    }
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
    locales: Vec<LocaleOption>,
    /// The deployment default locale, named in the inherit option.
    default_locale: String,
    /// Whether the operator currently inherits the default locale.
    inherits_locale: bool,
    /// The account's live sessions, most recently active first.
    sessions: Vec<SessionRow>,
    error: Option<String>,
}

/// One of the account's live sessions, formatted for the page.
struct SessionRow {
    id: String,
    started: String,
    last_seen: String,
    expires: String,
    /// "Chrome on macOS", or the raw agent when it is not one we name, or a
    /// dash when the request carried none.
    device: String,
    /// Where it signed in from, blank when the deployment records no address.
    ip_address: String,
    current: bool,
}

struct TzOption {
    name: String,
    selected: bool,
}

struct LocaleOption {
    /// The base language tag (`en`, `kn`).
    code: String,
    /// The language's own name, shown untranslated.
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
    icon: String,
}

/// Renders a template to an HTML response, mapping a render failure to a 500.
pub(crate) fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        // A render failure is unexpected: log the cause via AdminError, then mask.
        Err(e) => AdminError::from(e).into_response(),
    }
}

pub(crate) fn render_error() -> Response {
    error::masked_500()
}

pub(crate) fn not_found() -> Response {
    AdminError::NotFound.into_response()
}

pub(crate) fn forbidden() -> Response {
    AdminError::Forbidden.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_form() -> form::FormConfig {
        form::FormConfig {
            entity: "widgets".to_string(),
            title: "Widgets".into(),
            base_path: "/widgets".to_string(),
            id_field: "id".to_string(),
            fields: vec![],
            persist: None,
            timestamps: false,
        }
    }

    fn resource_with(form: Option<form::FormConfig>, permission: Option<&str>) -> Resource {
        Resource {
            base_path: "/widgets".to_string(),
            nav_label: "Widgets".into(),
            list: backend_users_list_config(),
            form,
            permission: permission.map(str::to_string),
        }
    }

    #[test]
    #[should_panic(expected = "must declare a permission")]
    fn write_resource_without_permission_aborts_boot() {
        // A create/edit form with no permission would expose its mutations to any
        // signed-in operator; mounting it aborts boot.
        let resource = resource_with(Some(write_form()), None);
        let _ = mount_resource(
            &resource,
            &field::builtin_registry(),
            &list::builtin_column_registry(),
            &std::collections::HashSet::new(),
            &persist::PersisterRegistry::new(),
            &[],
        );
    }

    #[test]
    #[should_panic(expected = "uses unregistered type `nope`")]
    fn unregistered_column_type_aborts_boot() {
        let mut resource = resource_with(None, None);
        let mut score = list::ListColumn::new("score", "Score");
        score.column_type = "nope".into();
        resource.list.columns.push(score);
        let _ = mount_resource(
            &resource,
            &field::builtin_registry(),
            &list::builtin_column_registry(),
            &std::collections::HashSet::new(),
            &persist::PersisterRegistry::new(),
            &[],
        );
    }

    #[test]
    fn read_only_resource_may_omit_permission() {
        // No write path, so no permission is required: this mounts without panicking.
        let resource = resource_with(None, None);
        let _ = mount_resource(
            &resource,
            &field::builtin_registry(),
            &list::builtin_column_registry(),
            &std::collections::HashSet::new(),
            &persist::PersisterRegistry::new(),
            &[],
        );
    }

    #[test]
    fn shell_localizes_through_the_translator_and_falls_back() {
        // The default translator has no catalog, so each source returns itself.
        let shell = Shell::test();
        assert_eq!(shell.t("Save"), "Save");
        assert_eq!(shell.tt(&Text::dynamic("Discard")), "Discard");
    }

    #[test]
    fn shell_tf_interpolates_integer_arguments() {
        let shell = Shell::test();
        assert_eq!(
            shell.tf("Page {page} of {pages}", &[("page", 2), ("pages", 5)]),
            "Page 2 of 5"
        );
    }

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

    #[test]
    fn accept_language_orders_by_weight_and_drops_wildcard() {
        assert_eq!(
            parse_accept_language("en-US,en;q=0.9,kn;q=0.8"),
            vec!["en-US", "en", "kn"]
        );
        // A higher-weight later tag wins; the wildcard is dropped.
        assert_eq!(
            parse_accept_language("en;q=0.5,kn,*;q=0.1"),
            vec!["kn", "en"]
        );
    }

    #[test]
    fn locale_chain_prefers_operator_then_header_then_default() {
        // A deployment with hi/kn/ta catalogs loaded (en is always serveable).
        let loaded = ["hi".to_string(), "kn".to_string(), "ta".to_string()];
        // Operator preference leads, then header order, then the default; en is
        // appended last as the source fallback, and nothing repeats.
        assert_eq!(
            resolve_locale_chain(Some("kn"), Some("ta;q=0.9,hi;q=0.8"), "en", &loaded),
            vec!["kn", "ta", "hi", "en"]
        );
        // An explicit en in the header keeps its signaled position, not forced last.
        assert_eq!(
            resolve_locale_chain(Some("kn"), Some("en;q=0.9"), "ta", &loaded),
            vec!["kn", "en", "ta"]
        );
        // A regional tag normalizes to its base language.
        assert_eq!(
            resolve_locale_chain(Some("kn-IN"), None, "en", &loaded),
            vec!["kn", "en"]
        );
    }

    #[test]
    fn locale_chain_skips_unserveable_and_defaults_to_en() {
        // Only hi has a catalog here; en is always serveable.
        let loaded = ["hi".to_string()];
        // Tags with no loaded catalog (and not en) are skipped; an unserveable
        // default falls to en, leaving just the source.
        assert_eq!(
            resolve_locale_chain(Some("fr"), Some("de,es"), "zz", &loaded),
            vec!["en"]
        );
        // No signal at all is still a valid en chain.
        assert_eq!(resolve_locale_chain(None, None, "en", &loaded), vec!["en"]);
        // A loaded catalog makes its locale serveable with no other change.
        assert_eq!(
            resolve_locale_chain(Some("hi"), None, "en", &loaded),
            vec!["hi", "en"]
        );
        // The pseudo-locale resolves without a catalog (QA selects it explicitly).
        assert_eq!(
            resolve_locale_chain(Some(laterite_core::PSEUDO_LOCALE), None, "en", &loaded),
            vec![laterite_core::PSEUDO_LOCALE, "en"]
        );
    }

    #[test]
    fn page_assets_dedupe_preserve_order_and_classify() {
        let mut reg = AssetRegistry::new();
        reg.insert(
            "a.js",
            AdminAsset {
                mime: "text/javascript; charset=utf-8",
                bytes: std::borrow::Cow::Borrowed(b"//"),
            },
        );
        reg.insert(
            "b.css",
            AdminAsset {
                mime: "text/css; charset=utf-8",
                bytes: std::borrow::Cow::Borrowed(b"body{}"),
            },
        );
        // A repeated key collapses to one, keeping first-seen order; the URL is
        // built from the base and names the content; css-vs-js follows the mime.
        let urls = asset_urls(&reg);
        let assets = page_assets(&["a.js", "b.css", "a.js"], "/admin", &reg, &urls);
        assert_eq!(assets.len(), 2);
        assert_eq!(
            assets[0].url,
            format!("/admin/assets/a.{}.js", http_cache::digest(b"//"))
        );
        assert!(!assets[0].css);
        assert_eq!(
            assets[1].url,
            format!("/admin/assets/b.{}.css", http_cache::digest(b"body{}"))
        );
        assert!(assets[1].css);
        // Different bytes must produce different URLs, which is the whole
        // reason these may be served as immutable.
        assert_ne!(assets[0].url, assets[1].url);
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
            trusted_proxies: Arc::new(Vec::new()),
            icons: Arc::new(laterite_core::icons::IconSet::new()),
            sprite_url: Arc::from(""),
            admin_path: Arc::from("/admin"),
            secure_cookie: false,
            origin: Arc::from(""),
            timezone: Tz::UTC,
            default_locale: Arc::from("en"),
            catalogs: Arc::new(CatalogStore::default()),
            app_name: "Configured Name".to_string(),
            brand_cache: Arc::new(RwLock::new(None)),
            field_types: Arc::new(field::builtin_registry()),
            column_types: Arc::new(list::builtin_column_registry()),
            plugin_defined: Arc::new(laterite_core::Registry::new()),
            assets: Arc::new(builtin_assets()),
            asset_urls: Arc::new(asset_urls(&builtin_assets())),
            pickers: Arc::new(picker::PickerRegistry::new()),
            overrides: Arc::new(field::NoOverrides),
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
            label: code.into(),
            description: String::new().into(),
            category: "General".into(),
            hint: None,
            order: 1,
            icon: None,
            permission: permission.map(str::to_string),
            link: None,
            fields: Vec::new(),
            route: None,
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
        let tr = Translator::new("en");
        let sidebar = |path: &str| {
            resolve_nav_context(
                &[],
                &items,
                "/admin",
                &superuser,
                path,
                &tr,
                icons::Icons::new(&laterite_core::icons::IconSet::new(), "/s.svg"),
            )
            .0
        };
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
        // A settings screen of the item's own activates that item. Branding is
        // the framework's one form-backed setting; unresolved in this test, it
        // falls back to a path built from its code's segments, which is a URL
        // rather than the dotted storage key it used to be.
        assert_eq!(
            active_path("/admin/settings/branding").as_deref(),
            Some("/admin/settings/branding")
        );
        // The dotted storage key is not a path at all any more, and neither is
        // the vendor-qualified form: the panel's own settings carry no vendor.
        assert_eq!(active_path("/admin/settings/laterite.brand"), None);
        assert_eq!(active_path("/admin/settings/laterite/brand"), None);
        // The dashboard is not a settings context, so it has no sidebar.
        assert!(sidebar("/admin").is_empty());
    }

    #[test]
    fn active_nav_lights_the_right_tab() {
        let nav = vec![
            NavLink {
                label: "Dashboard".into(),
                path: "/admin".to_string(),
                icon: Some("layout-dashboard"),
            },
            NavLink {
                label: "Pages".into(),
                path: "/admin/pages".to_string(),
                icon: None,
            },
            NavLink {
                label: "Settings".into(),
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

#[cfg(test)]
mod session_cookie_tests {
    use super::*;

    /// The session cookie always dies with the browser. Persistence belongs to
    /// the remember credential, which can be rotated and revoked on its own; a
    /// long-lived session cookie could be neither.
    #[test]
    fn the_session_cookie_never_outlives_the_browser() {
        let cookie = session_cookie("tok".into(), "/admin", false);
        assert_eq!(cookie.max_age(), None);
        assert!(cookie.http_only().unwrap_or(false));
        assert_eq!(cookie.path(), Some("/admin"));
    }

    /// "Stay signed in" is what survives a closed browser, and it carries its
    /// own lifetime rather than borrowing the session's.
    #[test]
    fn the_remember_cookie_carries_its_own_lifetime() {
        let credential = laterite_auth::RememberCredential {
            cookie: "sel:ver".into(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(14),
        };
        let cookie = remember_cookie(credential, "/admin", true);
        let age = cookie.max_age().expect("a remember cookie must persist");
        // Within a second of fourteen days, allowing for the clock read.
        assert!((age.whole_seconds() - 14 * 24 * 60 * 60).abs() <= 1);
        assert!(cookie.http_only().unwrap_or(false));
        assert!(cookie.secure().unwrap_or(false));
    }

    /// Clearing has to match on path or the browser keeps the original.
    #[test]
    fn the_removal_matches_the_cookie_it_clears() {
        let removal = remember_removal("/admin");
        assert_eq!(removal.name(), REMEMBER_COOKIE);
        assert_eq!(removal.path(), Some("/admin"));
    }
}
