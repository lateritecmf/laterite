# Third-Party Notices

A built binary carries the licence notices of everything compiled into it.

## Distribute source or a crate

Nothing to do: licence files travel inside each package.

## Distribute a binary

Generate a notice with
[`cargo about`](https://github.com/EmbarkStudios/cargo-about) and ship it
beside the binary:

```bash
cargo install cargo-about
cargo about init          # writes about.toml and about.hbs
cargo about generate about.hbs > NOTICE.html
```

It reads every dependency's `NOTICE`, `LICENSE` and `COPYING`, Laterite's
bundled fonts and icons included.

## What Laterite bundles

Work | Licence
--- | ---
IBM Plex Sans, IBM Plex Mono | SIL Open Font License 1.1
Space Grotesk | SIL Open Font License 1.1
Lucide, a subset | ISC, with an MIT annex for icons derived from Feather
htmx | Zero-Clause BSD

Full texts: `crates/admin/NOTICE`.
