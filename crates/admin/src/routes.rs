//! Routes a module mounts itself: admin screens, and public endpoints.
//!
//! A [`Resource`](crate::Resource) covers list and form screens as data. A
//! [`Screen`] is the substrate underneath: any routes a module wants, mounted
//! inside the admin with the same session, permission, CSRF and error handling a
//! resource gets. Reach for it only when descriptors cannot express the screen.
//!
//! A [`PublicRoute`] is the same idea outside the admin, for the endpoints that
//! have to live at a literal path: `/robots.txt`, a sitemap, a feed, a webhook.

use std::sync::Arc;

use axum::Router;
use laterite_core::{Db, Registry, Text};

/// What a contributed route is given at boot.
///
/// Deliberately narrow: the admin's own router state stays private, so its
/// composition can change without breaking a screen. Fields are read through
/// accessors, so this can gain context after 1.0; `new` cannot, and extra context
/// would arrive as a builder.
#[derive(Clone)]
pub struct RouteCtx {
    db: Db,
    base_path: Arc<str>,
    admin_path: Arc<str>,
    base_url: Arc<str>,
    plugin_defined: Arc<Registry>,
}

impl RouteCtx {
    pub(crate) fn new(
        db: Db,
        base_path: &str,
        admin_path: &str,
        base_url: &str,
        plugin_defined: Arc<Registry>,
    ) -> Self {
        Self {
            db,
            base_path: Arc::from(base_path),
            admin_path: Arc::from(admin_path),
            base_url: Arc::from(base_url),
            plugin_defined,
        }
    }

    pub fn db(&self) -> &Db {
        &self.db
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
pub struct ScreenReg {
    /// Path relative to the module's admin base, starting with a slash.
    pub base_path: String,
    /// Menu label, localized at render. `None` mounts it without a nav entry.
    pub nav_label: Option<Text>,
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
            permission: permission.into(),
            screen,
        }
    }

    /// Shows this screen in the menu under `label`.
    pub fn in_menu(mut self, label: impl Into<Text>) -> Self {
        self.nav_label = Some(label.into());
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
