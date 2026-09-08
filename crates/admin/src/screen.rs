//! Screens a module mounts itself.
//!
//! A [`Resource`](crate::Resource) covers list and form screens as data. A screen
//! is the substrate underneath: any routes a module wants, mounted inside the
//! admin with the same session, permission, CSRF and error handling a resource
//! gets. Reach for it only when descriptors cannot express the screen.

use std::sync::Arc;

use axum::Router;
use laterite_core::{Db, Text};

/// What a screen is given at boot.
///
/// Deliberately narrow: the admin's own router state stays private, so its
/// composition can change without breaking a screen. Fields are read through
/// accessors, so this can gain context after 1.0; `new` cannot, and extra context
/// would arrive as a builder.
#[derive(Clone)]
pub struct ScreenCtx {
    db: Db,
    base_path: Arc<str>,
}

impl ScreenCtx {
    pub(crate) fn new(db: Db, base_path: &str) -> Self {
        Self {
            db,
            base_path: Arc::from(base_path),
        }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Where this screen actually mounted, after the module namespace, any base
    /// it declared, the deployment's overrides and the admin mount.
    pub fn base_path(&self) -> &str {
        &self.base_path
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
    fn mount(&self, ctx: &ScreenCtx) -> Router;
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
            fn mount(&self, _ctx: &ScreenCtx) -> Router {
                Router::new()
            }
        }
        let reg = ScreenReg::new("/import", "acme.import", Arc::new(Noop));
        assert!(reg.nav_label.is_none());
        assert!(reg.in_menu("Import").nav_label.is_some());
    }
}
