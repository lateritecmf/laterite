//! Where blob bytes physically live, and the trait that hides which.
//!
//! Deliberately without a `list()`. The reference system's media browser needed
//! directory listing because its folders were disk directories; here folders are
//! database rows, so browsing never touches a driver. That removes listing costs,
//! per-file stat calls, and eventual-consistency surprises in one move.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;

use bytes::Bytes;
use futures::Stream;
use laterite_core::strata::async_trait;

/// Blob bytes in flight. Never a `Vec<u8>`: a file is streamed from the moment it
/// arrives to the moment it lands, so its size never has to fit in memory.
pub type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage io error: {0}")]
    Io(#[from] io::Error),
    /// A path that escapes the disk root, or is otherwise not one this driver
    /// will accept. Never reported with the offending path, which may be
    /// attacker-supplied.
    #[error("refused a path outside the disk")]
    BadPath,
}

/// What a driver knows about a stored blob without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobMeta {
    pub size: u64,
}

/// A place blobs are stored: the local filesystem, an object store.
#[async_trait]
pub trait StorageDriver: Send + Sync + 'static {
    /// Writes `data` at `path`, replacing anything there.
    ///
    /// Blob paths are derived from a content hash, so a replacement writes
    /// identical bytes and overwriting is always safe.
    async fn put(&self, path: &str, data: ByteStream) -> Result<(), StorageError>;

    async fn get(&self, path: &str) -> Result<ByteStream, StorageError>;

    /// Removes a blob. Missing is success: deletion is driven by garbage
    /// collection, which may run twice over the same unreferenced blob.
    async fn delete(&self, path: &str) -> Result<(), StorageError>;

    /// Size and existence, without reading the blob.
    async fn metadata(&self, path: &str) -> Result<Option<BlobMeta>, StorageError>;

    /// The blob's directly reachable URL, or `None` on a disk that has none.
    /// A private disk returns `None` and is served through the application.
    fn public_url(&self, path: &str) -> Option<String>;
}

/// Joins `path` under `root`, refusing anything that escapes it.
///
/// Blob paths are framework-derived rather than user-supplied, so this should
/// never fire. It exists because "should never" is not a security property, and
/// the cost of being wrong here is reading or writing any file the process can.
pub(crate) fn resolve(root: &Path, path: &str) -> Result<PathBuf, StorageError> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(StorageError::BadPath);
    }
    let mut out = root.to_path_buf();
    for part in candidate.components() {
        match part {
            Component::Normal(segment) => out.push(segment),
            // `..` is the escape; a root or prefix component means the path was
            // not relative after all; `.` is meaningless here.
            _ => return Err(StorageError::BadPath),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_may_not_escape_its_disk() {
        let root = Path::new("/srv/media");
        for bad in [
            "../etc/passwd",
            "blobs/../../etc/passwd",
            "/etc/passwd",
            "./blobs/ab/cd/hash",
        ] {
            assert!(
                matches!(resolve(root, bad), Err(StorageError::BadPath)),
                "{bad:?} was allowed"
            );
        }
    }

    #[test]
    fn an_ordinary_blob_path_resolves_under_the_root() {
        let root = Path::new("/srv/media");
        assert_eq!(
            resolve(root, "blobs/ab/cd/abcdef").unwrap(),
            Path::new("/srv/media/blobs/ab/cd/abcdef")
        );
    }
}
