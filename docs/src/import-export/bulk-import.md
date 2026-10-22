# Bulk Import

A resource's records imported from a CSV, each row written through the
resource's form, one commit a row, with a report. Built into the admin.

## Let a resource be imported into

```yaml
# descriptors/products.yaml
import: true
```

```yaml
import:
  permission: acme.import_products
```

```rust
Resource::new("/products", "Products", list)
    .form(form)
    .permission("acme.manage_products")
    .import(ImportConfig::new())
```

```toml
[backend]
storage = "storage"
```

```rust
fn register(&self, registry: &mut Registry) {
    registry.add(DiskReg::new("private", Arc::new(LocalDisk::new("/srv/acme/private"))));
}
```

Key | Default | Does
--- | --- | ---
`import: true` | off | Mounts the pages at `/admin/products/import` and shows an Import button beside the list's export menu.
`import.permission` | the resource's | The permission a user needs to import.
`backend.storage` | `storage` | The directory the uploaded file is kept under, in `private`, as a media record outside the library.
`DiskReg` named `private` | none | Keeps the file on another [disk](../services/storage-disks.md).

A resource declaring `import` needs a form and a permission; either missing
aborts boot naming the resource.

The delimiter is read off the file's first line (comma, semicolon or tab) and
can be set by hand. Files are read as UTF-8; a byte-order mark is accepted.

## Start a run from code

```rust
use laterite_admin::import::{start, Duplicates, Match, RunSpec};

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
`.keep_blank(bool)` | `true` | On an update, a blank matched cell leaves the record's value as it is. `false` clears the field.
`.dry_run(bool)` | `false` | Nothing is written; the counts say what an import would do.
`.first_row_titles(bool)` | `true` | The first row names the columns and is passed over.
`.delimiter(u8)` | `b','` | The byte between values.
`.titles(vec)` | none | The file's titles; a later file with the same titles is proposed this matching.
`.media(id)` | none | The stored payload's record, kept on the run.

`start` returns `RunError::Spec` for a match naming a field the form lacks, no
matches, or a duplicates key no column feeds. A blank row is passed over.

## Poll a run

```rust
use laterite_admin::import::{find, Status};

let run = find(ctx.db(), id).await?.expect("a run");
match run.status {
    Status::Running => format!("{} rows so far", run.processed),
    Status::Done => format!("{} created, {} updated", run.created, run.updated),
    Status::Failed => run.failure.unwrap_or_default(),
}
```

Field | Meaning
--- | ---
`status` | `Running`, `Done`, or `Failed` (the run itself stopped; `failure` says why). A run left running by a stopped process reads as failed.
`processed`, `created`, `updated`, `skipped`, `errors` | Row counts. On a dry run, what an import would do.
`notes` | One per message: `row` (the line in the file), `kind` (`Error` or `Skipped`), `field`, `message` (a `Text`, localized where it renders).
`spec` | What ran, `dry_run` included.
`resource`, `media_id`, `actor_label`, `started_at`, `finished_at` | What ran, as recorded.

A refused row is a note; the next row still runs.
