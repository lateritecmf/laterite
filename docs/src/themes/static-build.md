# Static Build

`lat theme build` writes a theme's pages and text assets to a directory any static host serves.

```bash
lat theme build acme
lat theme build acme --out public --base-url https://acme.example
```

Every page whose URL has no parameter is written at its URL (`/about` to `about/index.html`), the `/404` page also as `404.html`, a page whose URL names a file (`/robots.txt`, `/sitemap.xml`) as that file, and the text assets under `themes/<code>/assets/`. A clean URL's canonical link and `this.url` carry the trailing slash a static host serves it with (`/about/`). A page with parameters or a [hidden page](pages.md#hide-a-page) is skipped and named.

Flag | Default | Meaning
--- | --- | ---
`--out` | `dist` | The output directory.
`--base-url` | the application's `app.url`, else one derived from `server.listen` | The origin `url()` and the canonical link use.
`--from <DIR>` | | Build a theme folder with no application; the folder's name is the code. See [Static Website](../getting-started/static-website.md).
`--copy <SRC>:<DEST>` | none | A directory copied into the output at `<DEST>`. Repeatable.

## Preview a theme folder

```bash
lat theme serve --from themes/acme
lat theme serve --from themes/acme --listen 127.0.0.1:3000 --copy docs/book:docs
```

Flag | Default | Meaning
--- | --- | ---
`--from <DIR>` | required | The theme folder.
`--listen` | `127.0.0.1:8080` | The address.
`--copy <SRC>:<DEST>` | none | As `build --copy`.

## Load a theme folder from Rust

```rust
use lateritecmf::web::theme::{build, folder};

let loaded = folder::load(Path::new("themes/acme")).await?;
let report = build::build(&loaded.db, &loaded.code, Path::new("dist"), "https://acme.example").await?;
```

Field | Holds
--- | ---
`db` | A private in-memory database holding the theme.
`code` | The folder's name.
`report` | The `ImportReport` of the load.

## Build from Rust

```rust
use lateritecmf::web::theme::build;

let report = build::build(&db, "acme", Path::new("dist"), "https://acme.example").await?;
println!("{} pages, {} assets, {} skipped", report.pages.len(), report.assets.len(), report.skipped.len());
```

## Build from the application

`lat theme build` renders without an application's modules; a template that needs their [markup](../markup/templating.md#add-markup-from-a-module) or [components](components.md) is built from the application with `build::build_with`:

```rust
let report = build::build_with(
    &db,
    "acme",
    Path::new("dist"),
    "https://acme.example",
    &[Arc::new(Money)],
    &[&ComponentReg::new("acme.posts", "Posts", Arc::new(Posts))],
)
.await?;
```

Argument | Meaning
--- | ---
`markup` | The `Markup` registrations to install, in order.
`components` | The `ComponentReg` registrations the pages attach.

A site rendered by your own templating, with no theme, is covered under [Static Site Generation](static-sites.md).
