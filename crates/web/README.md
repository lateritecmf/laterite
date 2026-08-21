<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# laterite-web

The Laterite public web layer.

Where [`laterite_admin`](https://docs.rs/laterite-admin) renders the private
admin, this crate renders the public face of an application. Its first
capability is **static-site generation**: an application renders its pages to
HTML strings and hands them to a [`StaticSite`], which writes them as files,
copies static assets, and generates a `sitemap.xml` and `robots.txt`. The
output is a plain directory suitable for any static host or CDN.

Page templates live in the application (Askama, or any renderer that produces
a `String`); this crate owns the file layout, the sitemap, and the shared
[`Meta`] tags (title, description, canonical URL, Open Graph) so pages stay
consistent and shareable.

```rust
use laterite_web::{Meta, StaticSite};

let meta = Meta::new("Acme", "The Acme website.").canonical("https://acme.example/");
let home = render_home(&meta.head_tags());

let mut site = StaticSite::new("dist", "https://acme.example")?;
site.page("/", &home)?;
site.assets("static", "static")?;
site.finish()?;
```

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
