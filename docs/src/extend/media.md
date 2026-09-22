# Media and File Storage

`laterite-media` stores files: hashed on arrival, one copy per identical bytes,
each recorded as a row your tables point at. Optional.

```toml
[dependencies]
laterite-media = "0.8"
```

```rust
Bootstrap::new("config")
    .module(laterite_media::module())
    .serve()
```

Term | Meaning
--- | ---
Blob | Bytes, named by their BLAKE3 hash.
Record | A file with a stable id, pointing at a blob.
Link | Attaches a record to its owner.

Replacing a file makes a new blob; the record's id and every reference to it
stay.

## Take a file in

```rust
use laterite_media::{ingest, LocalDisk};

let disk = LocalDisk::new("storage/private");
let stored = ingest(&disk, stream, 10 * 1024 * 1024).await?;

let id = laterite_media::record::create(&db, NewMedia {
    disk: "private".into(),
    hash: stored.hash,
    size: stored.size as i64,
    content_type: stored.content_type,
    original_name: filename,
    in_library: false,
    created_by: Some(user.id),
}).await?;
```

`ingest(disk, stream, limit)` takes any byte stream: a browser upload, a
server-side fetch, a URL import. It hashes as the bytes pass, aborts and
discards past the limit, and sniffs the content type from the bytes.

## Receive an upload

```rust
async fn receive(mut upload: VerifiedUpload) -> impl IntoResponse {
    while let Some(field) = upload.next_field().await? {
        // the token field is already consumed and checked
    }
}
```

```html
<form method="post" enctype="multipart/form-data">
  <input type="hidden" name="_csrf" value="{{ csrf_token }}">
  <input type="file" name="file">
</form>
```

`_csrf` comes first in the form, or arrives as the `X-CSRF-Token` header. A
route that parses the upload without `VerifiedUpload` has its response
refused.

## Declare disks

```rust
let public = LocalDisk::new("storage/media").public_at("/storage/media");
let private = LocalDisk::new("storage/private");
```

Disk | Serving
--- | ---
Public | Files served directly at the path.
Private | No public URL; streamed through the application.

Each record stores its disk. Deduplication is per disk.

## Delete

```rust
if laterite_media::record::delete(&db, id).await? {
    disk.delete(&blob_path(&hash)).await?;
}
```

`delete` returns whether the blob is now unreferenced. Deleting a blob already
gone succeeds.

## Write a driver

```rust
#[async_trait]
impl StorageDriver for MyDisk {
    async fn put(&self, path: &str, data: ByteStream) -> Result<(), StorageError>;
    async fn get(&self, path: &str) -> Result<ByteStream, StorageError>;
    async fn delete(&self, path: &str) -> Result<(), StorageError>;
    async fn metadata(&self, path: &str) -> Result<Option<BlobMeta>, StorageError>;
    fn public_url(&self, path: &str) -> Option<String>;
}
```

No `list()`: folders are database rows. Write to a temporary path and move
into place.
