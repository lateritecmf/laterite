# Bulk Import

`laterite-import` writes a file's rows through a resource's form, one commit a
row, and keeps a report. Optional; the file itself is stored through
`laterite-media`.

```toml
[dependencies]
laterite-import = "0.9"
```

```rust
Bootstrap::new("config")
    .module(laterite_media::MediaModule)
    .module(laterite_import::ImportModule)
    .serve()
```

## Register what may be imported

```rust
use laterite_import::ImportReg;
use laterite_media::{DiskReg, LocalDisk};

fn register(&self, registry: &mut Registry) {
    registry.add(ImportReg::new("/products", "Products").permission("acme.manage_products"));
    registry.add(DiskReg::new("private", Arc::new(LocalDisk::new("storage/private"))));
}
```

Registration | Does
--- | ---
`ImportReg::new(resource, label)` | Offers the resource at `resource` (its path as the descriptor declares it) on the screen under `label`. The resource must mount a form.
`.permission(code)` | A permission the operator must hold besides `laterite.import.run`.
`DiskReg::new("private", driver)` | The disk uploaded files are stored on. Required; a boot without one aborts naming it.

The screen mounts at `/admin/import` under the **Import** menu entry and
requires `laterite.import.run`. An operator picks a resource, uploads a CSV,
sees each file column with a sample and the form field proposed for it, chooses
how existing records are found, and starts the run. The report page shows the
counts as they move and every refused or skipped row by its line in the file.

## Start a run

```rust
use laterite_import::{start, Duplicates, Match, RunSpec};

let products = ctx.form("/products").expect("the products resource");
let spec = RunSpec::new("/products", vec![Match::new(0, "sku"), Match::new(1, "name")])
    .duplicates(Duplicates::update("sku"))
    .media(payload_id);

let run = start(ctx.db(), products, actor, spec, disk.get(&path).await?).await?;
```

`start` records the run, detaches the work and returns the run's id. The
request that started it may end; the rows keep being written.

Option | Default | Does
--- | --- | ---
`RunSpec::new(resource, matches)` | | The resource's path as its descriptor declares it, and the column matches.
`Match::new(column, field)` | | File column `column` (from 0) fills `field`. A column may fill several fields.
`.duplicates(Duplicates::skip(key))` | none | A row whose `key` is already there is passed over and noted.
`.duplicates(Duplicates::update(key))` | none | A row whose `key` is already there updates that record; fields the file does not carry keep their values.
`.first_row_titles(bool)` | `true` | The first row names the columns and is passed over.
`.delimiter(u8)` | `b','` | The byte between values.
`.media(id)` | none | The stored payload's record, kept on the run.

`start` returns `RunError::Spec` for a match naming a field the form lacks, no
matches, or a duplicates key no column feeds. Rows are read as UTF-8. A blank
row is passed over.

## Poll a run

```rust
use laterite_import::{find, Status};

let run = find(ctx.db(), id).await?.expect("a run");
match run.status {
    Status::Running => format!("{} rows so far", run.processed),
    Status::Done => format!("{} created, {} updated", run.created, run.updated),
    Status::Failed => run.failure.unwrap_or_default(),
}
```

Field | Meaning
--- | ---
`status` | `Running`, `Done`, or `Failed` (the run itself stopped; `failure` says why).
`processed`, `created`, `updated`, `skipped`, `errors` | Row counts.
`notes` | One per message: `row` (the line in the file), `kind` (`Error` or `Skipped`), `field`, `message` (a `Text`, localized where it renders).
`resource`, `media_id`, `actor_label`, `started_at`, `finished_at` | What ran, as recorded.

Each row goes through the form's rules, field types, listeners, persister and
audit log. A refused row is a note; the next row still runs.
