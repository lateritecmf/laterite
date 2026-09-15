# Media and File Storage

`laterite-media` stores files: it hashes them as they arrive, keeps one copy of
identical bytes, and records each one as a row your own tables can point at.

It is optional. An application that stores no files never compiles it.

```toml
[dependencies]
laterite-media = "0.6"
```

```rust
Bootstrap::new("config")
    .module(laterite_media::module())
    .serve()
```

Registering the module brings its migrations with it. The admin does not depend
on this crate and has no feature flag for it: optional subsystems reach the admin
the way a plugin does, through the registry.

## Three things, kept apart

- A **blob** is bytes, named by their own BLAKE3 hash.
- A **record** is a file with a stable identity, pointing at a blob.
- A **link** attaches a record to whatever owns it.

Keeping them separate is what makes replacing a file cheap. New bytes mean a new
blob and a new hash, while the record's id, and every reference to it, stay
exactly where they were.

## Taking a file in

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

`ingest` takes a byte stream, not a request, and that is deliberate: a browser
upload, a server-side fetch of a file a user picked from their own cloud storage,
and a URL import are all just different ways to produce one. None of them needs a
pipeline of its own.

Three things it guarantees:

- **Nothing is buffered whole.** The hash is computed as the bytes pass, so a
  file never has to fit in memory and needs no second read to be named.
- **The limit stops the read.** Passing it aborts mid-stream and discards what
  was written, rather than accepting the bytes and judging them afterwards.
- **The content type is sniffed from the bytes.** A sender's declared type is a
  claim about a file it chose; believing it is how an HTML page gets served as an
  image.

## Uploads

A form that sends a file is read with `VerifiedUpload`, which checks the request
token while parsing:

```rust
async fn receive(mut upload: VerifiedUpload) -> impl IntoResponse {
    while let Some(field) = upload.next_field().await? {
        // the token field is already consumed and checked
    }
}
```

Put `_csrf` first in the form. It has to be first, because finding it anywhere
else would mean buffering everything ahead of it:

```html
<form method="post" enctype="multipart/form-data">
  <input type="hidden" name="_csrf" value="{{ csrf_token }}">
  <input type="file" name="file">
</form>
```

A scripted upload can send the token as the `X-CSRF-Token` header instead, and
then nothing is consumed to find it.

Taking the upload with a bare parser instead does not skip the check: the guard
notices the check never happened and **refuses the response**. Forgetting gives
you a broken route, not an unguarded one.

## Disks

A disk is a named place files live. A **public** disk serves its files directly;
a **private** disk has no public URL and is streamed through the application.

```rust
let public = LocalDisk::new("storage/media").public_at("/storage/media");
let private = LocalDisk::new("storage/private");
```

Each record stores the disk it lives on, so a file's location is knowable without
consulting configuration that may have moved on since it was written.

**Deduplication is scoped to a disk.** Identical bytes on a local disk and in a
bucket are separate physical copies, so deleting one does not free the other.

## Deleting

Removing a file is not removing its bytes, because a hash may back several
records. `delete` says whether the blob is now unreferenced:

```rust
if laterite_media::record::delete(&db, id).await? {
    disk.delete(&blob_path(&hash)).await?;
}
```

Deleting a blob that is already gone succeeds, so collection may safely run twice
over the same one.

## Writing a driver

A driver stores and fetches bytes. Note what is **not** on the trait: there is no
`list()`. Folders are database rows rather than directories, so browsing never
touches storage, which removes listing costs and consistency surprises.

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

Write to a temporary location and move it into place. The blob's path is its own
hash, so a partially written file at that path would be a permanent lie about its
contents.
