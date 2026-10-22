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

## Find a file

Control | Narrows to
--- | ---
Folder | The files in it.
Search | A name, across every folder.
Kind | `Image`, `Video`, `Audio`, `Document` or `Other`.
Sort | Newest, oldest, name or size.

## Manage folders and files

Action | Where | Needs
--- | --- | ---
Make, rename, move or remove a folder | The folder bar | `media.manage`; an empty folder only.
Retitle, rename or move a file | The file page | `media.manage`
Crop an image | The file page | `media.manage`; a new file `<stem>-crop.<ext>` beside it, or the file replaced.
Delete a file | The file page | `media.delete`, or `media.delete_linked` when records use it.

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
record::set_in_library(&db, id, true, None).await?;
```

Call | Refused when
--- | ---
`folder::create`, `folder::rename` | The name has a slash or a control character, or a sibling has it.
`folder::move_to` | The target is the folder or below it.
`folder::delete` | The folder still holds a file or a folder.

`Listing.search` looks across every folder; `Kind::of(content_type)` gives
`Image`, `Video`, `Audio`, `Document` or `Other`.
