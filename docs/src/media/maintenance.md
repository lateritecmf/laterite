# Maintenance

`lat media` removes files nothing links to and checks every stored file
against its record.

```sh
lat media gc                 # files outside the library that nothing links to
lat media gc --grace 1       # younger than one hour is left alone (default 24)
lat media gc --dry-run       # list, remove nothing
lat media verify             # check every stored file against its record
```

Flag | Default | Meaning
--- | --- | ---
`--grace <hours>` | `24` | A record younger than this is never garbage.
`--dry-run` | off | Prints `id`, disk, hash and name; removes nothing.
`--storage <dir>` | `backend.storage` | Where the `media` and `private` disks live.

## Collect garbage

A file is removed with the last record naming it. A record on a disk the
command cannot reach (one the application registered itself) is skipped and
counted. From code:

```rust
let report = laterite_media::gc::collect(&db, &disks, chrono::Duration::hours(24)).await?;
```

## Verify stored files

`verify` exits non-zero on a fault and prints one line per faulty record:
missing, content mismatch, or a size the record disagrees with. From code:

```rust
let report = laterite_media::verify::run(&db, &disks).await?;
```

## Delete a record

```rust
if laterite_media::record::delete(&db, id).await? {
    disk.delete(&blob_path(&hash)).await?;
}
```

`delete` returns whether the file is now unreferenced. Deleting a file already
gone succeeds.
