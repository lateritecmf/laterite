//! Laterite media: content-addressed blob storage with streaming ingest.
//!
//! Three concepts are kept apart, which is the whole design:
//!
//! - a **blob** is physical bytes, named by their own BLAKE3 hash;
//! - a **record** is a file with a stable identity, pointing at a blob;
//! - a **link** attaches a record to something that owns it.
//!
//! Conflating them is what makes a media library hard to replace a file in, or
//! to reference from two places. Here, replacing a file writes a new blob and a
//! new hash while the record's id and every reference to it stay put.
//!
//! This crate currently carries the bottom layer: the storage driver, the ingest
//! primitive, and the record table. The library browser, virtual folders,
//! variants and the finder widget arrive with the CMS work.

pub mod driver;
pub mod folder;
pub mod gc;
pub mod ingest;
pub mod kind;
pub mod library;
pub mod link;
pub mod local;
pub mod migrations;
pub mod record;
pub mod variant;
pub mod verify;

pub use driver::{BlobMeta, ByteStream, Disposition, StorageDriver, StorageError};
pub use folder::{Folder, FolderError};
pub use ingest::{blob_path, ingest, stream_of, IngestError, Ingested};
pub use kind::Kind;
pub use link::{Link, Owner};
pub use local::LocalDisk;
pub use record::{MediaRecord, NewMedia, NewMediaExtra, RecordError};

use std::sync::Arc;

use laterite_core::{MigrationSet, Module, ModuleId, Text};

/// A named disk the application declares, for a module that stores files to
/// find.
///
/// Registered once, from the application's own module:
/// `registry.add(DiskReg::new("private", Arc::new(LocalDisk::new("storage/private"))))`.
/// A module reads it through `RouteCtx::contributions::<DiskReg>()`, picks the
/// disk by name and stores that name on each record it writes, so a file's
/// location is knowable from the record alone.
#[non_exhaustive]
pub struct DiskReg {
    pub name: String,
    pub driver: Arc<dyn StorageDriver>,
    /// What the disk is for, shown where an operator chooses one. Defaults to
    /// the name.
    pub label: Text,
    /// A private disk's files are never served by a hash URL alone: a signed
    /// URL or an app-streamed token gates them.
    pub private: bool,
}

impl DiskReg {
    pub fn new(name: impl Into<String>, driver: Arc<dyn StorageDriver>) -> Self {
        let name = name.into();
        Self {
            label: Text::dynamic(name.clone()),
            name,
            driver,
            private: false,
        }
    }

    /// Marks the disk private (see [`DiskReg::private`]).
    pub fn private(mut self) -> Self {
        self.private = true;
        self
    }

    /// What the disk is for, localized where shown.
    pub fn label(mut self, label: impl Into<Text>) -> Self {
        self.label = label.into();
        self
    }
}

/// This crate's module. An application that stores files registers it on
/// `Bootstrap`; one that does not never compiles this crate at all.
///
/// Optional subsystems reach the admin the way a plugin does, through the
/// registry, so the admin never depends on them and needs no feature flag for
/// them. A descriptor naming something this module would have contributed, in an
/// application that did not register it, aborts boot naming the field and the
/// type rather than degrading silently.
pub fn module() -> Box<dyn Module> {
    Box::new(MediaModule)
}

/// The `laterite.media` module: the media record table.
pub struct MediaModule;

impl Module for MediaModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }

    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }
}
