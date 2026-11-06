# Soft Delete

Delete moves a record to the trash. A trashed record leaves every list, form
and export until it is restored or deleted permanently.

## Add the column

```rust
use laterite_core::strata::*;

Table::create()
    .table(Alias::new("notes"))
    .col(ColumnDef::new(Alias::new("id")).big_integer().not_null().auto_increment().primary_key())
    .col(ColumnDef::new(Alias::new("title")).text().not_null())
    .col(soft_delete_col())
    .to_owned()
```

`soft_delete_col()` is a nullable `deleted_at` column.

## Turn it on

```yaml
entity: notes
path: /notes
title: Notes
permission: acme.manage_notes
soft_delete: true
list:
  deletable: true
  columns:
    title: {}
```

```rust
Resource::new("/notes", t!("Notes"), list).soft_delete()
```

## The list

View | Buttons | Route
--- | --- | ---
The list | Delete moves the selection to the trash; Trash opens the trash | `POST {path}/delete`
The trash (`?trashed=1`) | Restore; Delete permanently; Leave the trash | `POST {path}/restore`, `POST {path}/purge`

A trashed record has no edit page. Delete permanently removes only records that
are in the trash.

A trash and a restore are saves: model listeners, `timestamps`, `footprints`
and the audit log see an update of `deleted_at`.

## Trash related rows with their parent

```yaml
relations:
  comments:
    entity: comments
    foreign_key: note_id
    soft_delete: true
    list:
      columns:
        body: {}
    form:
      fields:
        body: {}
```

Event | Related rows
--- | ---
The parent is trashed | Each live row takes the parent's `deleted_at` value
The parent is restored | Rows carrying that same value are restored; a row trashed on its own stays in the trash

The related table needs `soft_delete_col()` too. The section's own Delete
removes a row permanently.

## Leave trashed rows out of a picker

```rust
TableSource::new("notes", "id", "title").scope(Arc::new(SoftDelete))
```

A search skips trashed rows; a stored id still shows its label.
