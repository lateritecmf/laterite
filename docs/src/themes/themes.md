# Themes

A theme is a set of templates edited from the admin and served as the site; this page covers the module, the Themes screen, a theme's config and preview image, its templates and revisions, and the active and edit theme. Register the module and its tables migrate with the application's:

```rust
use lateritecmf::prelude::*;
use lateritecmf::web::theme::ThemeModule;

Bootstrap::new("config")
    .module(ThemeModule)
    .module(migrations::AppModule)
    .run()
    .await
```

Create a theme from its YAML config, add templates, and make it the site's theme:

```rust
use lateritecmf::web::theme::{self, store, Kind};

let site = store::create_theme(&db, "acme", "name: Acme\nauthor: Acme\n").await?;
store::save_template(&db, site.id, Kind::Layout, "default.htm", None, LAYOUT, None).await?;
store::save_template(
    &db,
    site.id,
    Kind::Page,
    "home.htm",
    Some(serde_json::json!({ "url": "/", "layout": "default" })),
    HOME,
    Some(operator_id),
)
.await?;
theme::set_active_theme(&db, "acme").await?;
```

## Mount the screen

`ThemeModule` registers the Themes tab at `/admin/themes` under the permission `web.manage_themes`, granted to the Administrator role.

Setting | Default | Meaning
--- | --- | ---
`backend.paths."laterite.theme/themes"` | `/themes` | Where the tab mounts under the admin, as a deployment override.

A module's own screen audits through the route context:

```rust
ctx.audit(&user, "acme.thing.save", Some("thing"), Some(&id), Some(&label), None).await;
```

## Write the config

```yaml
name: Acme
author: Acme
homepage: https://acme.example
description: The Acme site.
parent: base
require:
  - acme.blog
form:
  fields:
    site_name:
      label: Site name
      default: Acme
```

Key | Type | Default | Meaning
--- | --- | --- | ---
`name` | string | `""` | The theme's display name.
`author` | string | `""` | Who made it.
`homepage` | string | `""` | The author's site.
`description` | string | `""` | One line about the theme.
`preview` | string | none | A URL for the image on the theme list; the initials stand in without one.
`parent` | string | none | The theme whose templates fill in for the ones this theme lacks (see [Child Themes](child-themes.md)).
`require` | list of module ids | `[]` | The modules the theme's templates need.
`form` | form descriptor | none | The theme's own settings fields (see [Theme Settings](theme-settings.md) and [Form Descriptor](../forms/form-descriptor.md)).

A key outside this table is refused. `ThemeConfig::parse` reads the YAML and `ThemeConfig::to_yaml` writes it.

## Add a preview image

```yaml
name: Acme
preview: https://acme.test/themes/acme/preview.png
```

Property | Standard
--- | ---
Size | 1200 × 800 pixels. Minimum 600 × 400.
Aspect | 3:2, exactly.
Format | PNG or JPEG. WebP is accepted.
Weight | Under 300 KB.
Subject | The home page as the theme renders it, cropped from the top of the viewport at 1200 pixels wide.
Framing | No browser chrome, device frame, drop shadow, border or rounded corners: the image edges are the page edges.
Overlays | No added text, badges, logos or colour swatches.
Content | Representative content and real images; no placeholder blocks.
Colour scheme | Light, unless the theme is dark only.
Transparency | None: an opaque image.
Shown at | 184 pixels wide on the theme list, so headings and large shapes must still read at that size.

A theme without `preview` shows its initials on the theme list.

## Save a template

Kind | Path | Holds
--- | --- | ---
`Kind::Layout` | `layout` | The frame a page renders inside (see [Layouts and Partials](layouts-and-partials.md)).
`Kind::Page` | `page` | A page served at its URL (see [Pages](pages.md)).
`Kind::Partial` | `partial` | A fragment a layout or page includes.
`Kind::Content` | `content` | A block of HTML, Markdown or text (see [Content](content.md)).
`Kind::Asset` | `asset` | A text file the site serves (a stylesheet, a script).

A path is `/`-separated segments of lowercase letters, digits, dots, hyphens and underscores, at most 255 characters. `settings` is a JSON object; `None` keeps the current one.

```rust
// A theme's own templates, or those of one kind.
let all = store::list_templates(&db, site.id, None).await?;
let pages = store::list_templates(&db, site.id, Some(Kind::Page)).await?;

store::delete_template(&db, template.id).await?;
```

URL | Serves
--- | ---
`/themes/<code>/assets/<path>` | The theme's text asset, rendered, with an ETag and `public, no-cache`.

## Restore a revision

Every save keeps what it replaced:

```rust
let revisions = store::revisions(&db, template.id).await?; // newest first
let restored = store::restore_revision(&db, revisions[0].id, Some(operator_id)).await?;
```

## Manage themes

```rust
let all = store::list_themes(&db).await?;
let one = store::find_theme(&db, "acme").await?;
store::update_theme_config(&db, "acme", "name: Acme\nparent: base\n").await?;
store::set_locked(&db, "acme", true).await?;
let copy = store::duplicate_theme(&db, "acme", "acme-copy").await?;
let child = store::create_child_theme(&db, "acme", "acme-child", "Acme child").await?;
store::delete_theme(&db, "acme-copy").await?;
```

Call | Refused when
--- | ---
`create_theme` | The code is taken, is not lowercase letters, digits and hyphens, or the parent is missing or leads back to the theme.
`update_theme_config` | The theme is locked, or the parent is missing or leads back.
`save_template`, `delete_template` | The theme is locked, or the path is not a template path.
`delete_theme` | The theme is active, locked, or another theme's parent.

## Set the active and edit theme

```rust
theme::set_active_theme(&db, "acme").await?;
let live = theme::active_theme(&db).await?;

theme::set_edit_theme(&db, operator_id, Some("acme-next")).await?;
let editing = theme::edit_theme(&db, operator_id).await?; // the operator's choice, else the active theme
theme::set_edit_theme(&db, operator_id, None).await?;
```

A visitor sees the active theme. A user signed in to the admin with `web.manage_themes` sees their edit theme and its [hidden pages](pages.md#hide-a-page), and reads `this.preview` as true (see [Properties](../markup/properties.md)).

## Replace the site

`ThemeModule` serves the active theme's pages at every URL the admin and the other public routes leave. A module of your own takes the site's place through `PublicRouteReg::site`:

```rust
registry.add_public_route(PublicRouteReg::site(Arc::new(MySite)));
```

```rust
let operator = ctx.operator(&headers).await;     // the session cookie's operator, if any
ctx.record_unserved("GET", "/missing", 404).await; // into the request log
```
