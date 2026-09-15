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

pub use driver::{BlobMeta, ByteStream, StorageDriver, StorageError};
pub use ingest::{blob_path, ingest, stream_of, IngestError, Ingested};
pub use local::LocalDisk;
