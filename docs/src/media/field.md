# Media Field

A `media` field holds one file, or a list of files, picked from the
[library](library.md).

```yaml
fields:
  cover_id: { type: media, label: Cover, mode: image }
  gallery: { type: media, label: Gallery, max: 6 }
  fonts: { type: media, label: Fonts, mode: font, max: 0 }
  logo_id: { type: media, label: Logo, mode: image, public: true }
```

Option | Default | Meaning
--- | --- | ---
`mode` | `file` | `image` offers images alone and shows thumbnails; `font` offers font files alone.
`max` | `1` | How many files the field holds: `1` stores one id in a bigint column; any other number stores a JSON list in a text column, `0` meaning no limit.
`public` | `false` | The file is shown to people who are not signed in: the picker marks a private file and does not take it, a save refuses one, and a file the field holds cannot be made private.

A save links the picked files to the record under the field's name; a
settings screen links them to its code. The file page lists the screens and
fields that use a file, and deleting such a file needs `media.delete_linked`.

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
