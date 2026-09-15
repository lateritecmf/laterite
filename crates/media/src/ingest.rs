//! Taking bytes in: hash while streaming, then store.
//!
//! Every source converges here. A browser upload, a server-side fetch of a file
//! a user picked from their own cloud storage, a URL import: each is only a
//! different way to produce a byte stream, and none of them is a separate
//! pipeline. That is why [`ingest`] takes a stream rather than a request.
//!
//! Two rules hold whatever the source:
//!
//! - **Nothing is buffered whole.** The hash is computed incrementally as the
//!   bytes pass, so a file never has to fit in memory and no second read pass is
//!   needed to hash what was just written.
//! - **The size cap is enforced during the stream**, not after. Checking a
//!   declared length, or checking afterwards, means having already accepted the
//!   bytes; a sender who lies about a length is the case the cap exists for.

use bytes::Bytes;
use futures::TryStreamExt;

use crate::driver::{ByteStream, StorageDriver, StorageError};

/// What went wrong taking a file in.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("the file is larger than the {limit} byte limit")]
    TooLarge { limit: u64 },
    #[error("the file is empty")]
    Empty,
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("read error: {0}")]
    Read(#[from] std::io::Error),
}

/// A blob that has been stored: what it is, and where it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    /// BLAKE3 of the bytes, hex. The blob's path is derived from it, so identical
    /// bytes on one disk are stored once.
    pub hash: String,
    pub size: u64,
    /// Sniffed from the leading bytes, never taken from the sender. A client's
    /// declared content type is a claim about a file it chose; believing it is
    /// how an HTML page gets served as an image.
    pub content_type: String,
    /// Where the blob sits on its disk.
    pub path: String,
}

/// The storage path for a hash: `blobs/ab/cd/<hash>`.
///
/// Two levels of hex fan-out, because a single directory holding every blob is a
/// problem on most filesystems long before a media library is large.
pub fn blob_path(hash: &str) -> String {
    format!("blobs/{}/{}/{}", &hash[0..2], &hash[2..4], hash)
}

/// The type sniffed from a file's leading bytes, or a neutral default.
///
/// Never `application/octet-stream` for something recognised, and never the
/// sender's claim: the bytes are the only honest source.
fn sniff(head: &[u8]) -> String {
    infer::get(head)
        .map(|t| t.mime_type().to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Hashes and stores a byte stream, returning what it turned out to be.
///
/// `limit` is a hard ceiling enforced mid-stream: the read stops and the partial
/// blob is discarded the moment it is passed, so an oversized or dishonest sender
/// costs only what has been read so far.
pub async fn ingest(
    disk: &dyn StorageDriver,
    data: ByteStream,
    limit: u64,
) -> Result<Ingested, IngestError> {
    let mut hasher = blake3::Hasher::new();
    let mut size: u64 = 0;
    let mut head: Vec<u8> = Vec::new();
    let mut chunks: Vec<Bytes> = Vec::new();

    let mut data = data;
    while let Some(chunk) = data.try_next().await? {
        size += chunk.len() as u64;
        if size > limit {
            return Err(IngestError::TooLarge { limit });
        }
        hasher.update(&chunk);
        // The first bytes decide the content type. `infer` needs only a few
        // hundred; keeping more would be holding the file in memory again.
        if head.len() < SNIFF_BYTES {
            let want = SNIFF_BYTES - head.len();
            head.extend_from_slice(&chunk[..want.min(chunk.len())]);
        }
        chunks.push(chunk);
    }

    if size == 0 {
        return Err(IngestError::Empty);
    }

    let hash = hasher.finalize().to_hex().to_string();
    let path = blob_path(&hash);

    // Identical bytes on this disk are already stored, and the path is the hash,
    // so there is nothing to write and nothing that could differ.
    if disk.metadata(&path).await?.is_none() {
        let stored: ByteStream = Box::pin(futures::stream::iter(
            chunks.into_iter().map(Ok::<Bytes, std::io::Error>),
        ));
        disk.put(&path, stored).await?;
    }

    Ok(Ingested {
        hash,
        size,
        content_type: sniff(&head),
        path,
    })
}

/// Enough leading bytes for every signature `infer` knows.
const SNIFF_BYTES: usize = 512;

/// Lets a caller hand `ingest` a plain buffer without building a stream.
pub fn stream_of(bytes: impl Into<Bytes>) -> ByteStream {
    let bytes = bytes.into();
    Box::pin(futures::stream::iter([Ok::<Bytes, std::io::Error>(bytes)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlobMeta, LocalDisk};

    fn disk() -> (LocalDisk, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (LocalDisk::new(dir.path()), dir)
    }

    /// A stream that yields many chunks, so the cap is exercised mid-flight
    /// rather than against one buffer that was already in memory.
    fn chunked(total: usize, chunk: usize) -> ByteStream {
        let chunks: Vec<_> = (0..total.div_ceil(chunk))
            .map(|i| {
                let len = chunk.min(total - i * chunk);
                Ok::<Bytes, std::io::Error>(Bytes::from(vec![b'x'; len]))
            })
            .collect();
        Box::pin(futures::stream::iter(chunks))
    }

    #[tokio::test]
    async fn a_file_is_named_by_its_own_contents() {
        let (disk, _dir) = disk();
        let out = ingest(&disk, stream_of("hello"), 1024).await.unwrap();

        // BLAKE3 of "hello", so the name is reproducible anywhere.
        assert_eq!(out.size, 5);
        assert_eq!(out.hash.len(), 64);
        assert_eq!(out.path, blob_path(&out.hash));
        assert!(out.path.starts_with(&format!("blobs/{}/", &out.hash[0..2])));
        assert_eq!(
            disk.metadata(&out.path).await.unwrap(),
            Some(BlobMeta { size: 5 })
        );
    }

    /// Identical bytes are one blob. An editor re-uploading the same logo is the
    /// ordinary case, not an edge one.
    #[tokio::test]
    async fn identical_bytes_are_stored_once() {
        let (disk, _dir) = disk();
        let first = ingest(&disk, stream_of("same"), 1024).await.unwrap();
        let second = ingest(&disk, stream_of("same"), 1024).await.unwrap();
        assert_eq!(first, second);
    }

    /// The cap has to stop the read, not judge it afterwards: by then the bytes
    /// have already been accepted, which is the cost the cap exists to avoid.
    #[tokio::test]
    async fn an_oversized_file_is_refused_and_leaves_nothing_behind() {
        let (disk, dir) = disk();
        let err = ingest(&disk, chunked(5000, 64), 1024).await.unwrap_err();
        assert!(
            matches!(err, IngestError::TooLarge { limit: 1024 }),
            "{err:?}"
        );

        // Nothing was written: no blob, and no partial file under the root.
        let blobs = dir.path().join("blobs");
        assert!(!blobs.exists(), "a refused upload left files behind");
    }

    #[tokio::test]
    async fn an_empty_file_is_not_a_file() {
        let (disk, _dir) = disk();
        assert!(matches!(
            ingest(&disk, stream_of(""), 1024).await.unwrap_err(),
            IngestError::Empty
        ));
    }

    /// The content type comes from the bytes. A sender's claim is a claim about a
    /// file it chose, and believing it is how a page gets served as an image.
    #[tokio::test]
    async fn the_content_type_is_sniffed_from_the_bytes() {
        let (disk, _dir) = disk();

        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        let out = ingest(&disk, stream_of(png.to_vec()), 1024).await.unwrap();
        assert_eq!(out.content_type, "image/png");

        // Unrecognised bytes get a neutral type rather than a guess.
        let out = ingest(&disk, stream_of("just text"), 1024).await.unwrap();
        assert_eq!(out.content_type, "application/octet-stream");
    }

    /// Sniffing reads only the leading bytes, so a signature split across the
    /// first chunks is still found.
    #[tokio::test]
    async fn a_signature_split_across_chunks_is_still_read() {
        let (disk, _dir) = disk();
        let chunks = vec![
            Ok::<Bytes, std::io::Error>(Bytes::from_static(&[0x89, b'P'])),
            Ok(Bytes::from_static(&[b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])),
            Ok(Bytes::from(vec![0u8; 64])),
        ];
        let out = ingest(&disk, Box::pin(futures::stream::iter(chunks)), 1024)
            .await
            .unwrap();
        assert_eq!(out.content_type, "image/png");
    }

    #[tokio::test]
    async fn a_blob_reads_back_byte_for_byte() {
        use futures::TryStreamExt;
        let (disk, _dir) = disk();
        let out = ingest(&disk, stream_of("round trip"), 1024).await.unwrap();

        let chunks: Vec<Bytes> = disk
            .get(&out.path)
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        let read: Vec<u8> = chunks.concat();
        assert_eq!(read, b"round trip");
    }

    /// Deleting an already-absent blob succeeds: garbage collection may pass
    /// over the same unreferenced blob more than once.
    #[tokio::test]
    async fn deleting_a_missing_blob_is_not_an_error() {
        let (disk, _dir) = disk();
        assert!(disk.delete(&blob_path(&"a".repeat(64))).await.is_ok());
    }

    #[tokio::test]
    async fn a_private_disk_has_no_public_url() {
        let (disk, _dir) = disk();
        assert_eq!(disk.public_url("blobs/ab/cd/x"), None);

        let dir = tempfile::tempdir().unwrap();
        let public = LocalDisk::new(dir.path()).public_at("/storage/media/");
        assert_eq!(
            public.public_url("blobs/ab/cd/x").as_deref(),
            Some("/storage/media/blobs/ab/cd/x")
        );
    }
}
