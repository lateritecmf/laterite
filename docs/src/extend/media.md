# Media and File Storage

`laterite-media` stores files: hashed on arrival, one copy per identical bytes,
each recorded as a row your tables point at. Its table migrates with the admin;
the crate is a dependency only for code that calls it.

```toml
[dependencies]
laterite-media = "0.9"
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

fn register(&self, registry: &mut Registry) {
    registry.add(DiskReg::new("private", Arc::new(private)).label("Uploads kept out of the library"));
}
```

Disk | Serving
--- | ---
Public | Files served directly at the path.
Private | No public URL; streamed through the application.

Each record stores its disk. Deduplication is per disk. A module that stores
files reads the disks through `ctx.contributions::<DiskReg>()` and picks one by
name. The admin's import keeps uploads on `private`: the registered disk of that
name, else `private` under `backend.storage`.

## The library

The Media tab lists the records with `in_library` set, in virtual folders, with upload, search, a kind filter, sorting, folders to make, rename, move and remove, and a file page to retitle, rename, move and delete. Library uploads go to the disk named `public` (the registered one, else `media` under `backend.storage`).

Permission | Grants
--- | ---
`media.browse` | The tab.
`media.upload` | Upload into it.
`media.manage` | Rename, retitle, move, and make or remove folders.
`media.delete` | Delete a file no record links to.
`media.delete_linked` | Delete a file records still use; the file page lists them.

```rust
use laterite_media::library::{self, Listing, Sort};
use laterite_media::{folder, link, Kind, Owner};

let photos = folder::create(&db, None, "Photos").await?;
let year = folder::create(&db, Some(photos.id), "2026").await?;   // path /Photos/2026
folder::rename(&db, year.id, "2027").await?;
folder::move_to(&db, year.id, None).await?;
folder::delete(&db, year.id).await?;                                // empty folders only
let crumbs = folder::ancestors(&db, year.id).await?;

let page = library::list(&db, &Listing {
    folder: Some(photos.id),
    search: None,
    kind: Some(Kind::Image),
    sort: Sort::Newest,
    page: 1,
    per_page: 60,
}).await?;

let owner = Owner { owner_type: "acme_posts", owner_id: "7", field: "gallery" };
link::set(&db, owner, &[first, second]).await?;                     // in that order
let held = link::of_owner(&db, owner).await?;
let users = link::users_of(&db, first).await?;
link::clear_owner(&db, "acme_posts", "7").await?;

record::update_details(&db, id, Some("Our cat"), Some("A tabby")).await?;
record::rename(&db, id, "cat.png").await?;
record::move_to_folder(&db, id, Some(photos.id)).await?;
record::set_in_library(&db, id, true, None).await?;
```

Call | Refused when
--- | ---
`folder::create`, `folder::rename` | The name has a slash or a control character, or a sibling has it.
`folder::move_to` | The target is the folder or below it.
`folder::delete` | The folder still holds a file or a folder.

`Listing.search` looks across every folder; `Kind::of(content_type)` gives `Image`, `Video`, `Audio`, `Document` or `Other`.

## Serve

Every record serves at `/storage/media/<hash>/<name>`:

Header | Value
--- | ---
`ETag` | The hash; `If-None-Match` answers 304.
`Cache-Control` | `public, max-age=31536000, immutable`
`X-Content-Type-Options` | `nosniff`
`Content-Disposition` | `inline`, or `attachment` for SVG, HTML, script and any `Other` kind.

A record on a disk registered `.private()` serves only at a signed URL:

```rust
registry.add(DiskReg::new("vault", Arc::new(private)).private());
let url = laterite_admin::medialib::file_url(&record);
let timed = laterite_admin::medialib::signed_file_url(&ctx, &record, Duration::from_secs(3600));
// /storage/media/<hash>/<name>?e=<expiry>&t=<signature>
```

Part | Value
--- | ---
`e` | Expiry, seconds since the epoch.
`t` | HMAC-SHA256 over the hash and the expiry, with `backend.secret`.
`Cache-Control` | `private, no-store`
Bare, expired, altered or retargeted | 404

The admin's own links to a private file carry a one-hour signature.

## Collect and verify

```sh
lat media gc                 # files outside the library that nothing links to
lat media gc --grace 1       # younger than one hour is left alone (default 24)
lat media gc --dry-run       # list, remove nothing
lat media verify             # re-hash every stored file against its record
```

Flag | Default | Meaning
--- | --- | ---
`--grace <hours>` | `24` | A record younger than this is never garbage.
`--dry-run` | off | Prints `id`, disk, hash and name; removes nothing.
`--storage <dir>` | `backend.storage` | Where the `media` and `private` disks live.

A blob goes with the last record naming it. `verify` exits non-zero on a
fault and prints one line per faulty record: missing, hash mismatch, or a
size the row disagrees with. A record on a disk the command cannot reach (one
the application registered itself) is skipped and counted. From code:

```rust
let report = laterite_media::gc::collect(&db, &disks, chrono::Duration::hours(24)).await?;
let report = laterite_media::verify::run(&db, &disks).await?;
```

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
    async fn signed_url(&self, path: &str, expires_in: Duration, disposition: Disposition)
        -> Result<Option<String>, StorageError>;
}
```

No `list()`: folders are database rows. Write to a temporary path and move
into place. `signed_url` defaults to `None`: the framework then streams the
file itself behind its own token. A store that presigns returns the URL.
