//! The local-filesystem driver: the only one a default deployment compiles.

use std::path::{Path, PathBuf};

use futures::TryStreamExt;
use laterite_core::strata::async_trait;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::driver::{resolve, BlobMeta, ByteStream, StorageDriver, StorageError};

/// Blobs under a root directory, optionally reachable at a URL prefix.
pub struct LocalDisk {
    root: PathBuf,
    /// The public prefix a blob is served at, or `None` for a private disk.
    url: Option<String>,
}

impl LocalDisk {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            url: None,
        }
    }

    /// Serves this disk's blobs directly under `prefix`. A disk without one is
    /// private and is streamed through the application instead.
    pub fn public_at(mut self, prefix: impl Into<String>) -> Self {
        self.url = Some(prefix.into());
        self
    }
}

#[async_trait]
impl StorageDriver for LocalDisk {
    /// Writes to a temporary file beside the target and renames it into place.
    ///
    /// The rename is atomic on the same filesystem, so a reader never sees a
    /// partial blob and a crashed write leaves a stray temp file rather than a
    /// corrupt one at a valid path. That matters more here than usual: the path
    /// is the content hash, so a truncated file at it would be a permanent lie.
    async fn put(&self, path: &str, data: ByteStream) -> Result<(), StorageError> {
        let target = resolve(&self.root, path)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temp = target.with_extension(format!("part-{}", std::process::id()));

        let mut file = tokio::fs::File::create(&temp).await?;
        let mut data = data;
        let write = async {
            while let Some(chunk) = data.try_next().await? {
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            // Durable before the rename, so a crash cannot publish a name whose
            // bytes never reached the disk.
            file.sync_all().await?;
            Ok::<_, std::io::Error>(())
        }
        .await;

        if let Err(err) = write {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(err.into());
        }
        tokio::fs::rename(&temp, &target).await?;
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<ByteStream, StorageError> {
        let file = tokio::fs::File::open(resolve(&self.root, path)?).await?;
        Ok(Box::pin(ReaderStream::new(file)))
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        match tokio::fs::remove_file(resolve(&self.root, path)?).await {
            Ok(()) => Ok(()),
            // Already gone is the outcome asked for. Garbage collection may pass
            // over the same unreferenced blob twice.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn metadata(&self, path: &str) -> Result<Option<BlobMeta>, StorageError> {
        match tokio::fs::metadata(resolve(&self.root, path)?).await {
            Ok(m) => Ok(Some(BlobMeta { size: m.len() })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn public_url(&self, path: &str) -> Option<String> {
        let prefix = self.url.as_ref()?;
        Some(format!(
            "{}/{}",
            prefix.trim_end_matches('/'),
            path.trim_start_matches('/')
        ))
    }
}

impl std::fmt::Debug for LocalDisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalDisk")
            .field("root", &self.root)
            .field("public", &self.url.is_some())
            .finish()
    }
}

/// The directory this disk stores blobs under, for tests and diagnostics.
impl AsRef<Path> for LocalDisk {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}
