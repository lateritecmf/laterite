<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# laterite-cli

`lat`: the Laterite command-line tool.

It scaffolds and sets up an application (`lat new`), checks that a project is
ready to serve (`lat doctor`), and manages administrators. Creating the first
administrator and recovering access must be reliable and must never depend on
a configured mail server, so `admin reset-password` sets a password directly.
Every command reports what it did and returns a non-zero exit code on failure.

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
