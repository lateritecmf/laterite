//! The admin half: what the application registers, and the screen.

mod screen;

use std::sync::Arc;

use laterite_admin::routes::ScreenReg;
use laterite_admin::Permission;
use laterite_core::{t, Registry, Text};

/// A resource an operator may import into, registered by the application
/// (`registry.add(ImportReg::new("/products", "Products"))`). The resource
/// must mount a form; one that does not aborts boot naming it.
#[non_exhaustive]
pub struct ImportReg {
    /// The resource's path as its descriptor declares it, relative to the
    /// admin root.
    pub resource: String,
    /// What the screen calls it, localized at render.
    pub label: Text,
    /// A permission the operator must hold besides the screen's own, usually
    /// the resource's. `None` asks for the screen's permission alone.
    pub permission: Option<String>,
}

impl ImportReg {
    pub fn new(resource: impl Into<String>, label: impl Into<Text>) -> Self {
        Self {
            resource: resource.into(),
            label: label.into(),
            permission: None,
        }
    }

    /// A permission the operator must also hold to import into this resource.
    pub fn permission(mut self, code: impl Into<String>) -> Self {
        self.permission = Some(code.into());
        self
    }
}

/// The permission the screen requires.
pub const PERMISSION: &str = "laterite.import.run";

/// The name of the disk uploaded files are stored on. The application
/// registers a `laterite_media::DiskReg` under it; a boot without one aborts
/// naming this.
pub const DISK: &str = "private";

/// Registers the screen and its permission.
pub(crate) fn contribute(registry: &mut Registry) {
    registry.add(Permission::new(
        PERMISSION,
        t!("Import records"),
        t!("Import"),
    ));
    registry
        .add(ScreenReg::new("/", PERMISSION, Arc::new(screen::ImportScreen)).in_menu(t!("Import")));
}
