<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# laterite-media

Laterite media: content-addressed blob storage with streaming ingest.

Three concepts are kept apart, which is the whole design:

- a **blob** is physical bytes, named by their own BLAKE3 hash;
- a **record** is a file with a stable identity, pointing at a blob;
- a **link** attaches a record to something that owns it.

Conflating them is what makes a media library hard to replace a file in, or
to reference from two places. Here, replacing a file writes a new blob and a
new hash while the record's id and every reference to it stay put.

This crate currently carries the bottom layer: the storage driver, the ingest
primitive, and the record table. The library browser, virtual folders,
variants and the finder widget arrive with the CMS work.

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
