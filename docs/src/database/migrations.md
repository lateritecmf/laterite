# Migrations

A migration is a unit with a stable `name`, an `up` and an optional `down`; a
module lists its migrations in apply order.

## Write a migration

```rust
use laterite_core::strata::*;
```

One import brings the `Migration` trait, `async_trait`, the schema and query
builders, `Db`, and the column helpers. `lat make:migration <description>`
writes a new file with it.

One migration per file under `src/migrations/`, named
`m<NNNN>_<description>.rs`, declaring `pub struct Migration` with a stable
`name`, an `up` and an optional `down`. `mod.rs` lists them in apply order:

```rust
laterite_core::migration_set! {
    module_id: "acme.blog",
    m0001_create_posts,
    m0002_create_comments,
}
```

Append at the end. Never reorder or rename a shipped entry: applied migrations
are tracked by `(module_id, name)`.

## Use the column helpers

```rust
use laterite_core::{bool_col, key_col};

Table::create()
    .table(Article::Table)
    .col(key_col(Article::Slug).not_null().unique_key())
    .col(bool_col(Article::Published).not_null().default(0));
```

Helper | Description
--- | ---
`key_col(name)` | A key column: any id, code, token or column named in an index or foreign key. Chain `.not_null()`, `.primary_key()`, `.unique_key()` as usual. Use `.text()` only for unindexed prose.
`bool_col(name)` | A boolean column. Bind and read it as a `bool` through the [query helpers](queries.md). Chain `.not_null()`, `.default(0)` as usual.

Ids are `bigint` auto-increment; an insert never sets one. Keep a migration to
one schema change.
