# Media Library

The Media tab holds the files a site and its records use: folders, upload,
search, a kind filter and a page per file. Opening it takes `media.browse`.

Permission | Grants
--- | ---
`media.browse` | The tab.
`media.upload` | Upload into it.
`media.manage` | Rename, retitle, move and crop, and make or remove folders.
`media.delete` | Delete a file no record links to.
`media.delete_linked` | Delete a file records still use; the file page lists them.

Grant them from a role like any other [permission](../admin/permissions.md).

## Upload

```toml
# config/default.toml
[backend]
upload_extensions = ["jpg", "jpeg", "png", "webp", "svg", "pdf", "css"]
```

A file whose extension is not listed is refused; an empty list allows images,
documents, audio, video, fonts and archives. An SVG is cleaned on the way in.
Library uploads go to the disk named `public`, else `media` under
`backend.storage` (see [Storage Disks](../services/storage-disks.md)).
Ticking **Private** sends them to the disk named `private`, else `private`
under `backend.storage`.

File | Served at
--- | ---
Public | `/storage/media/<hash>/<name>`, cacheable.
Private | `/admin/media-files/<id>/<name>`, signed in, for whoever may browse the library or see a record the file is attached to; see [Storage disks](../services/storage-disks.md). A tile carries a lock.

## Find a file

Control | Narrows to
--- | ---
Folder | The files in it.
Search | A name, across every folder.
Kind | `Image`, `Video`, `Audio`, `Document`, `Font` or `Other`; `Font` is listed once the library holds a font.
Sort | Newest, oldest, name or size.

## Manage folders and files

Action | Where | Needs
--- | --- | ---
Make, rename, move or remove a folder | The folder bar | `media.manage`; an empty folder only.
Retitle, rename or move a file | The file page | `media.manage`
Resize an image | **Resize** on the file page: a width, a height or both, as a new file or in place | `media.manage`
Crop an image | The file page | `media.manage`; a new file `<stem>-crop.<ext>` beside it, or the file replaced.
Make private, make public | The file page | `media.manage`; the file moves disk and its thumbnails are made again.
Delete a file | The file page | `media.delete`, or `media.delete_linked` when records use it.
Delete a folder with its contents | The folder bar | `media.manage` and `media.delete`.
Move several files and folders | Tick them, then Move | `media.manage`
Delete several files and folders | Tick them, then Delete | `media.delete`; `media.manage` when a folder is ticked.

Selection | Result
--- | ---
Nothing ticked | Move and Delete are disabled.
Select all | Every file and folder on the page.
Shift-click | The run between the last tick and this one.
A ticked folder, Delete | Its files and folders go with it.
A file a record still uses, without `media.delete_linked` | Kept, with the folders that hold it.
A folder moved into itself | Refused; the others ticked still move.

## Work the library from code

```rust
use laterite_media::library::{self, Listing, Sort};
use laterite_media::{folder, record, Kind};

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

record::update_details(&db, id, Some("Our cat"), Some("A tabby")).await?;
record::rename(&db, id, "cat.png").await?;
record::move_to_folder(&db, id, Some(photos.id)).await?;
let has_fonts = library::has_kind(&db, Kind::Font).await?;
let unused = record::set_disk(&db, id, "private").await?;           // after copying the blob
record::set_in_library(&db, id, true, None).await?;
```

Call | Refused when
--- | ---
`folder::create`, `folder::rename` | The name has a slash or a control character, or a sibling has it.
`folder::move_to` | The target is the folder or below it.
`folder::delete` | The folder still holds a file or a folder.

`Listing.search` looks across every folder; `Kind::of(content_type)` gives
`Image`, `Video`, `Audio`, `Document` or `Other`.

## Hear a change

```rust
use laterite_admin::medialib::{FolderChange, MediaFolderChanged, MediaMoved, MediaRenamed, MediaRemoved, MediaStored};

struct Reindex;

#[async_trait]
impl Listener<MediaFolderChanged> for Reindex {
    async fn handle(&self, cx: &EventCx<'_>, event: &MediaFolderChanged) -> Result<(), EventError> {
        if event.change == FolderChange::Removed {
            acme::index::forget(cx.db(), &event.path).await?;
        }
        Ok(())
    }
}

registry.listen::<MediaFolderChanged>(Reindex);
```

Event | Name | Fields
--- | --- | ---
`MediaStored` | `media.stored` | `id`, `hash`, `name`, `by`
`MediaRemoved` | `media.removed` | `id`, `hash`, `name`, `by`
`MediaRenamed` | `media.renamed` | `id`, `name`, `was`, `by`
`MediaMoved` | `media.moved` | `id`, `folder_id`, `by`
`MediaFolderChanged` | `media.folder.changed` | `id`, `path`, `change` (`created`, `renamed`, `moved`, `removed`), `by`
