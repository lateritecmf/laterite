<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# laterite-import

Laterite bulk import: a file's rows written through a resource's form, with
a report.

A run reads a CSV, maps its columns onto a form's fields and writes each row
the way the form's own submit would: the same rules, field types, listeners,
persister and audit entry, one commit a row. The run is detached from the
request that started it and keeps its counts and notes in a table a screen
polls, so a long file finishes whether or not the browser waits, and a
refused row is a line in the report, never a stopped import.

A resource declaring `import: true` gets the pages under its own path: a
file taken in, its columns shown beside the form's fields with a match
proposed for each, a check that writes nothing, then the run and its report
as it fills. The application keeps the uploaded file on the disk it
registered as a `laterite_media::DiskReg` named `private`. From code,
[`start`] and [`find`] do the same without the pages.

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
