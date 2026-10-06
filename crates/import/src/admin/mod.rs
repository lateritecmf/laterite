//! The admin half: the pages mounted under a resource that declares `import`.

mod screen;

use std::sync::Arc;

use laterite_admin::routes::ResourceFeatureReg;
use laterite_core::Registry;

/// The name of the disk uploaded files are stored on. The application
/// registers a `laterite_media::DiskReg` under it; a boot without one aborts
/// naming this.
pub const DISK: &str = "private";

/// Registers the pages for the `import` descriptor key.
pub(crate) fn contribute(registry: &mut Registry) {
    registry.add(ResourceFeatureReg::new(
        "import",
        Arc::new(screen::ImportFeature),
    ));
}
