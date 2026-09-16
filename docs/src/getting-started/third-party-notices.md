# Third-Party Notices

Your application's binary contains work that neither you nor Laterite wrote: the
Rust crates it links, and the fonts, icons and scripts the admin panel bundles.
Most of that work is under licences that ask for a copyright notice to travel
with it. MIT says the notice must appear "in all copies"; the SIL Open Font
License asks that a copy of the licence accompany the font; Apache-2.0 accepts
the notice in the distribution, in documentation, or in a display the program
generates.

None of them asks for a screen in your admin panel. They ask that the text
accompany what you hand out.

## If you distribute source, or a crate

Nothing to do. Each crate's own licence files travel inside its package, and
Laterite's bundled assets are covered by `NOTICE` files inside the crates that
bundle them.

## If you distribute a built binary

You owe your users a notice covering everything inside it. Generate one with
[`cargo about`](https://github.com/EmbarkStudios/cargo-about), which reads your
dependency tree and gathers each crate's licence files:

```bash
cargo install cargo-about
cargo about init          # writes about.toml and about.hbs
cargo about generate about.hbs > NOTICE.html
```

Ship the result beside the binary: in the archive, in the container image, in
the package. That is how the rest of the ecosystem does it, and it is why
`ripgrep`, `gh` and `deno` have no `--licenses` flag.

**Laterite's bundled fonts and icons come along for free.** They are recorded in
`crates/admin/NOTICE`, and `cargo about` finds files named `NOTICE`, `LICENSE`
and `COPYING` in every dependency's source. You do not have to know that Laterite
bundles IBM Plex to end up crediting it.

## What Laterite bundles

For reference, in case you are auditing rather than generating:

| Work | Licence |
| --- | --- |
| IBM Plex Sans, IBM Plex Mono | SIL Open Font License 1.1 |
| Space Grotesk | SIL Open Font License 1.1 |
| Lucide (icon subset) | ISC, with an MIT annex for icons derived from Feather |
| htmx | Zero-Clause BSD, which asks for no notice |

The full texts are in `crates/admin/NOTICE`.

## Embedding the notice instead

You can compile the text into your binary and print it from your own CLI, the
way a browser offers an about-credits page. It is not required, and it is not
free: a notice covering a few hundred crates is a substantial amount of text,
and it costs noticeably more binary than the text itself once the linker has
padded it. Measure it before assuming it is negligible.

Shipping the file beside the binary costs nothing and satisfies the same terms.
Embed only if your binary genuinely travels alone, with no archive, package or
image around it to carry a second file.
