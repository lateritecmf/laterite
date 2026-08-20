# Static Site Generation

Laterite renders two faces of an application. The admin (from `laterite-admin`)
is the private, server-rendered back office. The public face is rendered by
`laterite-web`, whose first capability is static-site generation: an application
renders its public pages to HTML at build time and writes them to a directory
that any static host or CDN can serve.

This suits a marketing site, documentation, or a mostly-read content site: pages
are known at build time, so there is nothing to run in production but a file
server.

This is provided by the `laterite-web` crate.

```toml
[dependencies]
laterite-web = "0.1"
```

## The mental model

`laterite-web` owns the file layout, not the templating. Your application turns
data into an HTML `String` however it likes (Askama, or any renderer), and hands
each page to a `StaticSite`. The crate writes the files, copies your assets, and
generates a `sitemap.xml` and `robots.txt`.

Two types carry the whole flow:

- `Meta` builds the shared `<head>` tags (title, description, canonical URL, and
  Open Graph / Twitter card) so every page is described and shareable the same
  way.
- `StaticSite` collects rendered pages, copies static assets, and finishes by
  writing the sitemap and robots file.

## A minimal generator

A generator is an ordinary binary. It renders each page, writes it under its URL
path, copies the `static/` directory, and finishes:

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
    site.finish()?;
    Ok(())
}
```

Run it with `cargo run`. The output lands in `dist/`, ready to deploy.

## Clean URLs

`StaticSite::page` maps a URL path to a clean-URL file, so links have no `.html`
suffix:

| Path passed to `page` | File written | Served as |
| --- | --- | --- |
| `/` | `dist/index.html` | `/` |
| `/features/` | `dist/features/index.html` | `/features/` |
| `/get-started` | `dist/get-started/index.html` | `/get-started/` |

Every path passed to `page` is also recorded for the sitemap.

## Page metadata

`Meta` renders the `<head>` tags every page shares. All values are escaped, so
titles and descriptions are safe to build from content:

```rust
let meta = Meta::new(title, description)
    .canonical(format!("{base_url}{path}"))
    .image(format!("{base_url}/static/img/card.png"));

// Embed the result inside your document's <head>.
let head = meta.head_tags();
```

`canonical` also becomes the Open Graph URL; `image` becomes the social-share
image. Both are optional.

## Assets, sitemap, and robots

`assets(from, to)` copies a directory of CSS, fonts, and images into the output,
recursively. `finish()` writes a `sitemap.xml` listing every page you added and a
`robots.txt` that points at it. Call `finish()` once, after the last page:

```rust
site.assets("static", "static")?;
site.finish()?;
```

## Deploying

The output directory is plain files with no runtime, so any static host serves
it. A typical setup builds the generator and publishes `dist/`. On a host that
builds from a Git repository, a build command of `cargo run` and a publish
directory of `dist` is enough; commit the source and let the host produce the
output.

## What stays on the server

Static generation covers pages whose content is known at build time. Anything
that depends on the request (a form submission, a search box, per-user content,
the admin itself) stays on the live server. Pre-render the content-facing pages,
keep the interactive parts served, and decide the split per page. An application
whose core is auth-gated or write-heavy is served by the live server; static
generation is for its public, content-facing pages.
