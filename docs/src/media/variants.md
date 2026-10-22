# Image Variants

A preset names a size and format an image serves at; the variant renders on
its first request.

```rust
use laterite_media::{Format, Preset};

registry.add(Preset::new("card").width(600).height(400).crop());
registry.add(Preset::new("hero").width(1600).format(Format::WebP).quality(85));
```

```html
<img src="/storage/media/{{ hash }}/card/{{ name }}" width="600" height="400">
```

Builder | Default | Meaning
--- | --- | ---
`Preset::new(name)` | | The name in the URL.
`.width(px)`, `.height(px)` | unbounded | The box.
`.fit()` | yes | Inside the box, keeping the aspect ratio; a smaller source stays its size.
`.crop()` | | Filling the box, cropped from the centre.
`.exact()` | | The box, stretched.
`.format(Format::Auto | Jpeg | Png | WebP)` | `Auto` | `Auto` is JPEG for an opaque image and PNG for a transparent one; `WebP` is lossless.
`.quality(1..=100)` | `82` | JPEG quality.

## Override a built-in preset

Built in | Box | Used by
--- | --- | ---
`thumb` | 320 × 320, fit | The library grid.
`preview` | 1200 × 1200, fit | The file page.

A registered preset with a built-in name replaces it. An undeclared name is a
404. A file that is not an image serves as it is under any preset.

## Keep within the limits

Limit | Value
--- | ---
Side | 12 000 pixels.
Pixels | 50 million.

An image past either limit serves as it is. A variant renders upright,
whatever orientation the camera recorded; `width` and `height` are recorded
on upload.

## Render from code

```rust
let prepared = laterite_media::image::prepare(bytes);
let rendered = laterite_media::image::render(&bytes, &preset, prepared.orientation)?;
let made = variants.ensure(&db, disk.as_ref(), &record, &preset).await?;
```

## Crop an image

```
POST /admin/media/<id>/crop   x=100&y=50&w=200&h=100&mode=new
```

Field | Meaning
--- | ---
`x`, `y`, `w`, `h` | The area kept, in the image's own pixels, clamped to it.
`mode` | `new` (the default): a file named `<stem>-crop.<ext>` in the same folder. `replace`: this file changes in place and its variants are dropped.

The format is kept (PNG, JPEG, WebP; anything else becomes PNG). The file page
offers the dialog on an image to a user with `media.manage`.

```rust
let out = laterite_media::image::crop(&bytes, Rect { x, y, width, height }, orientation)?;
laterite_media::record::replace_blob(&db, id, &hash, size, &content_type, width, height).await?;
```
