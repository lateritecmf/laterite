<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# lateritecmf

Laterite CMF: the one crate an application depends on.

The framework is a family of crates with one version. This crate carries no
code of its own: it re-exports each crate under a short name and forwards the
database driver features, so an application names one dependency, pins one
version, and reaches everything through one path.

```toml
[dependencies]
lateritecmf = { version = "0.9", features = ["postgres"] }
```

```rust
use lateritecmf::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Bootstrap::new("config").module(AppModule).serve().await
}
```

A plugin depends on the crates it uses directly (`laterite-core`,
`laterite-admin`), which keeps its dependency light; the umbrella is for the
application that assembles them.

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
