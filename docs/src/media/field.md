# Media Field

A `media` field holds one file, or a list of files, picked from the
[library](library.md).

```yaml
fields:
  cover_id: { type: media, label: Cover, mode: image }
  gallery: { type: media, label: Gallery, max: 6 }
```

Option | Default | Meaning
--- | --- | ---
`mode` | `file` | `image` offers images alone and shows thumbnails.
`max` | `1` | How many files the field holds: `1` stores one id in a bigint column; any other number stores a JSON list in a text column, `0` meaning no limit.

A save links the picked files to the record under the field's name. The file
page lists the records that use a file, and deleting such a file needs
`media.delete_linked`.

Event | Detail
--- | ---
`lat:media:changed` | `{ids}`, after a pick, a removal or a clear.

## Link records from code

```rust
use laterite_media::{link, Owner};

let owner = Owner { owner_type: "acme_posts", owner_id: "7", field: "gallery" };
link::set(&db, owner, &media_ids).await?;                           // in that order
let held = link::of_owner(&db, owner).await?;
let users = link::users_of(&db, first).await?;
link::clear_owner(&db, "acme_posts", "7").await?;
```

Call | Does
--- | ---
`link::set(db, owner, media_ids)` | Replaces what the owner's field holds with `media_ids`, in that order.
`link::of_owner(db, owner)` | The ids the owner's field holds.
`link::users_of(db, id)` | Every owner linking the file.
`link::clear_owner(db, owner_type, owner_id)` | Drops every link a record holds.
