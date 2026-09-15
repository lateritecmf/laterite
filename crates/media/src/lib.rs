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
pub mod ingest;
pub mod local;
pub mod migrations;
pub mod record;

pub use driver::{BlobMeta, ByteStream, StorageDriver, StorageError};
pub use ingest::{blob_path, ingest, stream_of, IngestError, Ingested};
pub use local::LocalDisk;
pub use record::{MediaRecord, NewMedia, RecordError};

use laterite_core::{MigrationSet, Module, ModuleId};

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
