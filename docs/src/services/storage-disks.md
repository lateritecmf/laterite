# Storage Disks

A disk holds files; a record points at one and your tables point at the
record. The media table migrates with the admin; the crate is a dependency
only for code that calls it.

```toml
[dependencies]
laterite-media = "0.9"
```

Term | Meaning
--- | ---
Blob | The bytes of a file.
Record | A file with a stable id, pointing at a blob.
Link | Attaches a record to its owner.

Replacing a file makes a new blob; the record's id and every reference to it
stay.

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

A module that stores files reads the disks through
`ctx.contributions::<DiskReg>()` and picks one by name. The admin's import
keeps uploads on `private`: the registered disk of that name, else `private`
under `backend.storage`. The library uploads to `public`, else `media` there.

## Add an S3 disk

```toml
[dependencies]
laterite-media = { version = "0.9", features = ["s3"] }
```

```rust
use laterite_media::{DiskReg, S3Config, S3Disk};

let config: S3Config = settings.get("storage.disks.s3")?;
let disk = Arc::new(S3Disk::from_config(&config).await);
registry.add(DiskReg::new("s3", disk));
```

```toml
[storage.disks.s3]
bucket = "acme-media"
region = "auto"
endpoint = "https://<account>.r2.cloudflarestorage.com"
access_key = "..."
secret_key = "..."
prefix = "media"
public_url = "https://media.acme.test"
```

Key | Default | Meaning
--- | --- | ---
`bucket` | | The bucket.
`region` | `auto` | The region name.
`endpoint` | AWS | The store's URL (R2, MinIO, B2, Spaces, Wasabi).
`access_key`, `secret_key` | the SDK's default chain | Static credentials.
`prefix` | `""` | A key prefix every blob goes under.
`public_url` | none | The origin blobs are reachable at unsigned; unset, the disk is private and serves through presigned URLs.
`path_style` | `true` with an endpoint | `endpoint/bucket/key` addressing.

`signed_url` presigns a GET, `presigned_put` a PUT for a [direct
upload](../media/direct-uploads.md), and `copy` is server-side.

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
server-side fetch, a URL import. It aborts and discards past the limit, and
sniffs the content type from the bytes.

## Receive an upload

```rust
async fn receive(mut upload: VerifiedUpload) -> impl IntoResponse {
    while let Some(field) = upload.next_field().await? {
        // the token field is already consumed and checked
    }
}
```

```html
<form method="post" action="{{ action }}" enctype="multipart/form-data">
  <input type="hidden" name="_csrf" value="{{ shell.csrf_token() }}">
  {% let upload_id = "file" %}
  {% let upload_accept = ".csv" %}
  {% let upload_multiple = false %}
  {% let upload_url = String::new() %}
  {% let upload_fields = String::new() %}
  {% let upload_direct = String::new() %}
  {% let upload_hint = shell.t("A CSV file.") %}
  {% include "_upload.html" %}
  <button class="lat-btn lat-btn--primary" type="submit" data-upload-submit>{{ shell.t("Upload") }}</button>
</form>
```

`_csrf` comes first in the form, or arrives as the `X-CSRF-Token` header. A
route that parses the upload without `VerifiedUpload` has its response
refused.

Variable | Value
--- | ---
`upload_id` | The input's id, for its label.
`upload_accept` | An `accept` list, or empty.
`upload_multiple` | `true` for several files.
`upload_url` | Empty: the form submits the file. A URL: the zone posts each file there itself, with `_csrf` first and `X-Requested-With: fetch`, shows its progress, and reloads once every file is stored.
`upload_fields` | Fields sent before each file (`folder=3;kind=image`).
`upload_direct` | Empty, or the URL of a [direct upload](../media/direct-uploads.md) route the zone tries first.
`upload_hint` | A line under the prompt.
`data-upload-submit` | On the form's button: hidden while the zone sends.

A route the zone posts to answers JSON:

```json
{"stored": [{"id": 7, "name": "cover.png", "href": "/admin/media/7", "url": "/storage/media/<hash>/cover.png"}],
 "refused": [{"name": "huge.mov", "reason": "Larger than the upload limit."}]}
```

Event | Detail
--- | ---
`lat:upload:picked` | `{count}`, in a form.
`lat:upload:before-send` | `{count}`, cancelable.
`lat:upload:stored`, `lat:upload:failed` | `{name, reason}` per file.
`lat:upload:done` | `{failed}` once every file answered.

## Check what comes in

```toml
# config/default.toml
[backend]
upload_extensions = ["jpg", "jpeg", "png", "webp", "svg", "pdf", "css"]
```

```rust
use laterite_media::sanitize;

if !sanitize::extension_allowed(&name, ctx.upload_extensions()) {
    return refused(t!("This file type is not allowed."));
}
let bytes = match sanitize::svg(&bytes) {
    Some(drawing) => drawing.into_bytes(),
    None if sanitize::is_svg(&bytes) => return refused(t!("The image could not be read.")),
    None => bytes,
};
```

Function | Does
--- | ---
`extension_allowed(name, list)` | The name's extension, lowercased, is in the list. A name with none is refused.
`is_svg(head)` | The leading bytes are an SVG document, past any prolog, comment or doctype.
`svg(bytes)` | The SVG as a drawing: script, foreign content, event handlers, external references and `javascript:` URLs removed; `style` cleaned; the namespace present. `None` when it is not an SVG.
`css(text)` | A stylesheet without imports, script URLs, expressions, bindings or a closing tag.
`DEFAULT_EXTENSIONS` | The list when `upload_extensions` is empty: images, documents, audio, video, fonts, archives.

The library's own upload, direct upload and media field run these checks.

## Serve a file

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
`t` | A signature over the hash and the expiry, made with `backend.secret`.
`Cache-Control` | `private, no-store`
Bare, expired, altered or retargeted | 404

The admin's own links to a private file carry a one-hour signature.

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

There is no `list()`. Write to a temporary path and move into place.
`signed_url` defaults to `None`: the framework then streams the file itself
behind its own token. A store that presigns returns the URL.
