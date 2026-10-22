# Static Build

`lat theme build` writes a theme's pages and text assets to a directory any static host serves.

```bash
lat theme build acme
lat theme build acme --out public --base-url https://acme.example
```

Every page whose URL has no parameter is written at its URL (`/about` to `about/index.html`), the `/404` page also as `404.html`, and the text assets under `themes/<code>/assets/`. A page with parameters or a [hidden page](pages.md#hide-a-page) is skipped and named.

Flag | Default | Meaning
--- | --- | ---
`--out` | `dist` | The output directory.
`--base-url` | the application's `app.url`, else one derived from `server.listen` | The origin `url()` and the canonical link use.

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
