<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# laterite-macros

Translation-string macros.

`t!`, `tn!`, `tp!` build a `laterite_core::i18n::Text` at the call site. The
source is a string literal (so extraction can collect it), its `{name}`
placeholders are checked against the named arguments at compile time, and
positional `{}` or format specs are rejected. For a non-literal source, use
`Text::dynamic`.

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
