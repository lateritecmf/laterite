# Static Site Generation

`laterite-web` renders an application's public pages to HTML at build time,
into a directory any static host serves.

```toml
[dependencies]
laterite-web = "0.7"
```

Type | Role
--- | ---
`Meta` | The shared `<head>` tags: title, description, canonical URL, Open Graph and Twitter card.
`StaticSite` | Collects rendered pages, copies assets, and reports what it wrote.

Templating is yours: any renderer that produces an HTML `String`.

## Write a generator

```rust
use laterite_web::{Meta, StaticSite};

fn render_home(head: &str) -> String {
    format!("<!doctype html><html><head>{head}</head><body><h1>Acme</h1></body></html>")
}

fn main() -> std::io::Result<()> {
    let base_url = "https://acme.example";

    let meta = Meta::new("Acme", "The Acme website.")
        .canonical(format!("{base_url}/"))
        .image(format!("{base_url}/static/card.png"));
    let home = render_home(&meta.head_tags());

    let mut site = StaticSite::new("dist", base_url)?;
    site.page("/", &home)?;
    site.assets("static", "static")?;
    Ok(())
}
```

`cargo run` writes `dist/`.

## Map URLs to files

Path passed to `page` | File written | Served as
--- | --- | ---
`/` | `dist/index.html` | `/`
`/features/` | `dist/features/index.html` | `/features/`
`/get-started` | `dist/get-started/index.html` | `/get-started/`

## Set page metadata

```rust
let meta = Meta::new(title, description)
    .canonical(format!("{base_url}{path}"))
    .image(format!("{base_url}/static/img/card.png"));

let head = meta.head_tags();
```

Method | Description
--- | ---
`new(title, description)` | Escaped; safe to build from content.
`canonical(url)` | Also the Open Graph URL. Optional.
`image(url)` | The social-share image. Optional.

## Publish extra files

```rust
let urls: Vec<String> = site
    .paths()
    .iter()
    .map(|path| format!("{}{path}", site.base_url()))
    .collect();

site.file("sitemap.xml", &your_sitemap(&urls))?;
site.file("robots.txt", "User-agent: *\nAllow: /\n")?;
```

Method | Description
--- | ---
`paths()` | Every page written, in order.
`base_url()` | The prefix for absolute URLs.
`file(path, content)` | Writes verbatim at the output root. Creates parent directories; refuses a path outside the output.
`assets(from, to)` | Copies a directory recursively.

No `robots.txt` or `sitemap.xml` is written for you.

## Deploy

`dist/` is plain files. On a host that builds from Git: build command
`cargo run`, publish directory `dist`.

A page whose content depends on the request (a form, a search, per-user
content, the admin) stays on the live server.
