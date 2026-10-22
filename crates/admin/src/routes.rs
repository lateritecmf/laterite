//! Routes a module mounts itself: admin screens, and public endpoints.
//!
//! A [`Resource`](crate::Resource) covers list and form screens as data. A
//! [`Screen`] is the substrate underneath: any routes a module wants, mounted
//! inside the admin with the same session, permission, CSRF and error handling a
//! resource gets. Reach for it only when descriptors cannot express the screen.
//!
//! A [`PublicRoute`] is the same idea outside the admin, for the endpoints that
//! have to live at a literal path: `/robots.txt`, a sitemap, a feed, a webhook.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use laterite_auth::{AuditEntry, AuthService, AuthenticatedUser};
use laterite_core::{Db, Events, Registry, Text};

use crate::form::{FormField, FormHandle, Forms, ValueForm};
use crate::list::ImportLink;

/// What a contributed route is given at boot.
///
/// Deliberately narrow: the admin's own router state stays private, so its
/// composition can change without breaking a screen. Fields are read through
/// accessors, so this can gain context after 1.0; `new` cannot, and extra context
/// would arrive as a builder.
#[derive(Clone)]
pub struct RouteCtx {
    db: Db,
    auth: AuthService,
    parameters: laterite_core::parameters::Parameters,
    upload_extensions: Arc<Vec<String>>,
    field_types: Arc<crate::field::FieldRegistry>,
    overrides: Arc<dyn crate::field::OverrideResolver>,
    signing_key: Arc<Vec<u8>>,
    storage_dir: Arc<str>,
    base_path: Arc<str>,
    admin_path: Arc<str>,
    base_url: Arc<str>,
    plugin_defined: Arc<Registry>,
    events: Events,
    forms: Forms,
}

impl RouteCtx {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        db: Db,
        auth: AuthService,
        parameters: laterite_core::parameters::Parameters,
        upload_extensions: Arc<Vec<String>>,
        field_types: Arc<crate::field::FieldRegistry>,
        overrides: Arc<dyn crate::field::OverrideResolver>,
        signing_key: Arc<Vec<u8>>,
        storage_dir: &str,
        base_path: &str,
        admin_path: &str,
        base_url: &str,
        plugin_defined: Arc<Registry>,
        events: Events,
        forms: Forms,
    ) -> Self {
        Self {
            db,
            auth,
            parameters,
            upload_extensions,
            field_types,
            overrides,
            signing_key,
            storage_dir: Arc::from(storage_dir.trim_end_matches('/')),
            base_path: Arc::from(base_path),
            admin_path: Arc::from(admin_path),
            base_url: Arc::from(base_url),
            plugin_defined,
            events,
            forms,
        }
    }

    /// Appends one audit entry for a mutation `user` just performed through a
    /// module's own screen. A failed write is logged and never fails the
    /// operation it records.
    pub async fn audit(
        &self,
        user: &AuthenticatedUser,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        target_label: Option<&str>,
        detail: Option<&str>,
    ) {
        let actor = laterite_core::Actor::user(user.user.id, user.user.username.clone());
        let mut entry = AuditEntry::new(&actor, action);
        if let (Some(kind), Some(id)) = (target_type, target_id) {
            entry = entry.target(kind, id, target_label);
        }
        if let Some(detail) = detail {
            entry = entry.detail(detail);
        }
        if let Err(e) = self.auth.record_audit(entry).await {
            tracing::error!(action, error = %e, "failed to write audit log entry");
        }
    }

    /// A form over a JSON object from `fields`, rendered and collected through
    /// the registered field types; `scope` names what an override is scoped to.
    /// Refused, naming the field, when a type is unregistered or an option
    /// malformed: the fields may come from data edited at runtime.
    pub fn value_form(&self, fields: &[FormField], scope: &str) -> Result<ValueForm, Text> {
        ValueForm::new(
            fields,
            self.field_types.clone(),
            self.overrides.clone(),
            scope,
        )
    }

    /// The operator a request's session cookie belongs to, when it carries a
    /// live one: how a public route tells an operator from a visitor.
    pub async fn operator(&self, headers: &axum::http::HeaderMap) -> Option<AuthenticatedUser> {
        let token = headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|line| line.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(name, _)| *name == crate::SESSION_COOKIE)
            .map(|(_, value)| value.trim().to_string())?;
        self.auth.verify_session(&token).await.ok()
    }

    /// Counts a URL the site could not serve in the request log, as the panel
    /// does for its own.
    pub async fn record_unserved(&self, method: &str, path: &str, status: u16) {
        crate::requestlog::record(&self.db, method, path, status).await;
    }

    /// The file extensions an upload may carry (`backend.upload_extensions`,
    /// else [`laterite_media::sanitize::DEFAULT_EXTENSIONS`]), lowercased. A
    /// screen that takes files checks a name with
    /// [`laterite_media::sanitize::extension_allowed`] before storing it.
    pub fn upload_extensions(&self) -> &[String] {
        &self.upload_extensions
    }

    /// The deployment's storage directory (`backend.storage`), where a module
    /// keeps files when the application registered no disk for it.
    pub fn storage_dir(&self) -> &str {
        &self.storage_dir
    }

    /// Signs `message` with the deployment's key (HMAC-SHA256, lowercase hex).
    /// The key is `backend.secret`, else one generated at the first boot and
    /// kept in the settings table.
    pub fn sign(&self, message: &[u8]) -> String {
        sign_with(&self.signing_key, message)
    }

    /// Whether `signature` is [`sign`](Self::sign) of `message`, compared in
    /// constant time.
    pub fn verify_signature(&self, message: &[u8], signature: &str) -> bool {
        verify_with(&self.signing_key, message, signature)
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The parameters store, cached per process. See
    /// [`laterite_core::parameters`].
    pub fn parameters(&self) -> &laterite_core::parameters::Parameters {
        &self.parameters
    }

    /// A resource's form, to write its records from code: an importer, a sync,
    /// anything that saves what an operator could have typed.
    ///
    /// `path` is the resource's path as its descriptor declares it, relative to
    /// the admin root (`/products`). `None` when no resource mounts there or the
    /// one that does has no form. The handle is `Clone` and owns what it needs,
    /// so a detached task may keep it.
    pub fn form(&self, path: &str) -> Option<FormHandle> {
        self.forms
            .get(path)
            .map(|form| FormHandle::new(self.db.clone(), form.clone()))
    }

    /// The application's event bus, for a route that announces a fact:
    /// `ctx.events().emit(&Published { article_id }).await`. Clone it into the
    /// route's state.
    pub fn events(&self) -> &Events {
        &self.events
    }

    /// The site's own origin, without a trailing slash: the configured
    /// `app.url`, falling back to the bind address when none is declared.
    ///
    /// For the absolute URLs a route has to emit rather than link: a `<loc>` in a
    /// sitemap, a canonical URL, an entry in a feed. The request's `Host` header
    /// is not a substitute, because behind a proxy it is whatever the proxy sent.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The contributions of type `T` that the framework itself does not consume.
    ///
    /// This is how one module reads another's contributions. A module declaring
    /// its own kind of extension point defines a type, other modules contribute
    /// it from their `register`, and the route reads them here: the framework
    /// never learns the type, and registration order does not matter because
    /// everything is collected before any route runs.
    pub fn contributions<T: Send + Sync + 'static>(&self) -> Vec<&T> {
        self.plugin_defined.items::<T>()
    }

    /// Where this screen actually mounted, after the module namespace, any base
    /// it declared, the deployment's overrides and the admin mount.
    pub fn base_path(&self) -> &str {
        &self.base_path
    }

    /// Where the admin panel is mounted, after `backend.path`. Ask rather than
    /// assuming `/admin`: an operator may have moved it, and a module building a
    /// link into the panel, or excluding it from a sitemap, needs the real one.
    pub fn admin_path(&self) -> &str {
        &self.admin_path
    }

    /// An absolute admin URL under this screen. Build every self-link this way:
    /// a deployment can move the screen, so a hardcoded path would break.
    pub fn url(&self, relative: &str) -> String {
        join(&self.base_path, relative)
    }

    /// A context built by hand, for testing a route without booting the framework.
    ///
    /// `mount` takes a `RouteCtx`, and the framework's own constructor is private,
    /// so without this a plugin could only exercise its routes through a full boot.
    /// With it, a route is an ordinary `tower` service:
    ///
    /// ```ignore
    /// let ctx = RouteCtx::builder(db).admin_path("/backoffice").build();
    /// let response = MyRoute.mount(&ctx).oneshot(request).await?;
    /// ```
    ///
    /// This is also how the context grows after 1.0: a new field gets a builder
    /// method and a default, which `new` could not have offered.
    pub fn builder(db: Db) -> RouteCtxBuilder {
        RouteCtxBuilder {
            db,
            auth: None,
            base_path: "/".to_string(),
            admin_path: "/admin".to_string(),
            base_url: "http://localhost".to_string(),
            plugin_defined: None,
            events: None,
        }
    }
}

/// Builds a [`RouteCtx`]. Every field has a default, so a test names only what it
/// asserts on.
pub struct RouteCtxBuilder {
    db: Db,
    auth: Option<AuthService>,
    base_path: String,
    admin_path: String,
    base_url: String,
    plugin_defined: Option<Arc<Registry>>,
    events: Option<Events>,
}

impl RouteCtxBuilder {
    /// Where the screen mounted. Defaults to `/`.
    pub fn base_path(mut self, path: impl Into<String>) -> Self {
        self.base_path = path.into();
        self
    }

    /// Where the panel mounted. Defaults to `/admin`, which is what a route
    /// excluding the panel should *not* be allowed to assume: set it to something
    /// else in a test that checks the exclusion is read rather than hardcoded.
    pub fn admin_path(mut self, path: impl Into<String>) -> Self {
        self.admin_path = path.into();
        self
    }

    /// The site's origin. Defaults to `http://localhost`.
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// The contributions `RouteCtx::contributions` will read. Defaults to empty.
    pub fn contributions(mut self, registry: Arc<Registry>) -> Self {
        self.plugin_defined = Some(registry);
        self
    }

    /// The bus `RouteCtx::events` will hand out. Defaults to one nothing
    /// listens on; pass one built with a listener to assert on what a route
    /// announces.
    pub fn events(mut self, events: Events) -> Self {
        self.events = Some(events);
        self
    }

    /// The auth service [`RouteCtx::audit`] writes through. Without one, a
    /// default service over the same database.
    pub fn auth(mut self, auth: AuthService) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn build(self) -> RouteCtx {
        let events = self.events.unwrap_or_else(|| Events::new(self.db.clone()));
        let auth = self.auth.unwrap_or_else(|| {
            AuthService::new(self.db.clone(), laterite_auth::AuthConfig::default())
        });
        RouteCtx::new(
            self.db.clone(),
            auth,
            laterite_core::parameters::Parameters::new(self.db),
            Arc::new(crate::upload_extensions(&[])),
            Arc::new(crate::field::builtin_registry()),
            Arc::new(crate::field::NoOverrides),
            Arc::new(crate::random_key()),
            "storage",
            &self.base_path,
            &self.admin_path,
            &self.base_url,
            self.plugin_defined
                .unwrap_or_else(|| Arc::new(Registry::new())),
            events,
            Arc::new(HashMap::new()),
        )
    }
}

fn join(base: &str, relative: &str) -> String {
    let base = base.trim_end_matches('/');
    let relative = relative.trim_start_matches('/');
    if relative.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{relative}")
    }
}

/// A module's own admin screen.
pub trait Screen: Send + Sync + 'static {
    /// The routes this screen serves, relative to its own base. Handlers capture
    /// what they need from `ctx`, so the router carries no framework state.
    fn mount(&self, ctx: &RouteCtx) -> Router;
}

/// A registered screen: where it mounts, what it is called, and who may reach it.
///
/// Built with [`ScreenReg::new`] plus its builder methods. Non-exhaustive so the
/// framework can learn something new about a screen after 1.0 without a major bump:
/// anything it needs to know belongs here rather than as another trait method, which
/// is also what keeps the code half replaceable at the plugin boundary.
#[non_exhaustive]
pub struct ScreenReg {
    /// Path relative to the module's admin base, starting with a slash.
    pub base_path: String,
    /// Menu label, localized at render. `None` mounts it without a nav entry.
    pub nav_label: Option<Text>,
    /// Where the entry sits: under a section, with an icon, at a position.
    pub menu: crate::nav::Placement,
    /// The permission every route under this screen requires. Required, because
    /// an unguarded screen exposes whatever it serves to any signed-in operator.
    pub permission: String,
    pub screen: Arc<dyn Screen>,
}

impl ScreenReg {
    pub fn new(
        base_path: impl Into<String>,
        permission: impl Into<String>,
        screen: Arc<dyn Screen>,
    ) -> Self {
        Self {
            base_path: base_path.into(),
            nav_label: None,
            menu: crate::nav::Placement::default(),
            permission: permission.into(),
            screen,
        }
    }

    /// Shows this screen in the menu under `label`.
    pub fn in_menu(mut self, label: impl Into<Text>) -> Self {
        self.nav_label = Some(label.into());
        self
    }

    /// Where the menu entry sits. See [`Placement`](crate::nav::Placement).
    pub fn menu(mut self, placement: crate::nav::Placement) -> Self {
        self.menu = placement;
        self
    }
}

/// A module's own route outside the admin.
///
/// Public by definition: no permission gate and no admin chrome, though the
/// styled error pages still apply. A route that accepts a POST declares its own
/// stance on CSRF, because the admin's blanket protection assumes a session.
pub trait PublicRoute: Send + Sync + 'static {
    /// The routes this endpoint serves, relative to its declared path.
    fn mount(&self, ctx: &RouteCtx) -> Router;
}

/// A registered public route.
///
/// The path is literal and never namespaced: `/robots.txt` has to be exactly
/// that. Collisions are therefore likelier than in the admin, so two claims on
/// one path, or a claim on the admin mount, abort the boot.
/// Built with [`PublicRouteReg::new`]. Non-exhaustive for the same reason as
/// [`ScreenReg`].
#[non_exhaustive]
pub struct PublicRouteReg {
    pub path: String,
    pub route: Arc<dyn PublicRoute>,
}

impl PublicRouteReg {
    pub fn new(path: impl Into<String>, route: Arc<dyn PublicRoute>) -> Self {
        Self {
            path: path.into(),
            route,
        }
    }

    /// The site itself: the route every URL reaches that no admin route and no
    /// other public route serves, in place of the panel's own 404. One module
    /// may register it; a second aborts the boot as a clash.
    pub fn site(route: Arc<dyn PublicRoute>) -> Self {
        Self::new("/", route)
    }

    /// Whether this registration is the site (see [`PublicRouteReg::site`]).
    pub fn is_site(&self) -> bool {
        self.path == "/"
    }
}

/// A feature a module mounts under every resource whose descriptor declares
/// its key: the routes behind `import: true`, say.
///
/// The admin owns the key and the button; the module owns the routes. A
/// descriptor declaring a key no module serves aborts boot naming it, so the
/// admin never depends on the module.
pub trait ResourceFeature: Send + Sync + 'static {
    /// The routes, relative to `{resource}/{key}`, which is `ctx.base_path()`.
    fn mount(&self, ctx: &RouteCtx, resource: &ResourceInfo) -> Router;
}

/// A registered feature, keyed by the descriptor key it serves. Mounted under
/// every resource declaring that key, behind the key's permission or, when the
/// key names none, the resource's.
#[non_exhaustive]
pub struct ResourceFeatureReg {
    pub key: String,
    pub feature: Arc<dyn ResourceFeature>,
}

impl ResourceFeatureReg {
    pub fn new(key: impl Into<String>, feature: Arc<dyn ResourceFeature>) -> Self {
        Self {
            key: key.into(),
            feature,
        }
    }
}

/// The resource a feature mounts under, as the feature sees it.
#[derive(Clone)]
#[non_exhaustive]
pub struct ResourceInfo {
    /// The resource's path as its descriptor declares it, relative to the
    /// admin root (`/products`).
    pub path: String,
    /// Where its list mounted (`/admin/products`).
    pub base_path: String,
    /// The resource's menu label.
    pub label: Text,
    /// The permission the feature's routes are gated by.
    pub permission: String,
    /// The resource's form, to write its records through.
    pub form: FormHandle,
}

impl ResourceInfo {
    pub(crate) fn new(
        path: String,
        base_path: String,
        label: Text,
        permission: String,
        form: FormHandle,
    ) -> Self {
        Self {
            path,
            base_path,
            label,
            permission,
            form,
        }
    }
}

/// The link a list shows to a feature mounted under it.
pub(crate) fn feature_link(base_path: &str, key: &str, permission: &str) -> ImportLink {
    ImportLink {
        href: format!("{base_path}/{key}"),
        permission: permission.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_self_link_builds_from_where_the_screen_actually_mounted() {
        let base = "/admin/rainmill/location/import";
        assert_eq!(
            join(base, "/step2"),
            "/admin/rainmill/location/import/step2"
        );
        assert_eq!(join(base, "step2"), "/admin/rainmill/location/import/step2");
        assert_eq!(join(base, ""), "/admin/rainmill/location/import");
    }

    #[test]
    fn a_moved_screen_moves_its_links_with_it() {
        assert_eq!(join("/admin/places", "/step2"), "/admin/places/step2");
    }

    #[test]
    fn a_registration_defaults_to_no_menu_entry() {
        struct Noop;
        impl Screen for Noop {
            fn mount(&self, _ctx: &RouteCtx) -> Router {
                Router::new()
            }
        }
        let reg = ScreenReg::new("/import", "acme.import", Arc::new(Noop));
        assert!(reg.nav_label.is_none());
        assert!(reg.in_menu("Import").nav_label.is_some());
    }
}

pub(crate) fn sign_with(key: &[u8], message: &[u8]) -> String {
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("any key length");
    mac.update(message);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn verify_with(key: &[u8], message: &[u8], signature: &str) -> bool {
    use hmac::Mac;
    let Some(given) = unhex(signature) else {
        return false;
    };
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("any key length");
    mac.update(message);
    mac.verify_slice(&given).is_ok()
}

/// Bytes from lowercase or uppercase hex; `None` on an odd length or a stray
/// character.
pub(crate) fn unhex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod signing_tests {
    use super::*;

    #[test]
    fn a_signature_verifies_against_its_key_alone() {
        let sig = sign_with(b"key-one", b"message");
        assert_eq!(sig.len(), 64);
        assert!(verify_with(b"key-one", b"message", &sig));
        assert!(!verify_with(b"key-two", b"message", &sig));
        assert!(!verify_with(b"key-one", b"other", &sig));
        assert!(!verify_with(b"key-one", b"message", "zz"));
        assert!(!verify_with(b"key-one", b"message", &sig[..62]));
    }
}
