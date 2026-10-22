# Portability

Laterite runs on Postgres, MySQL or SQLite, chosen by the connection URL;
migrations and queries are written once.

## Use portable types

You write | Helper
--- | ---
`bool` | `bool_col` in schema; binds as `bool`; read with `AnyRowExt::get_bool`
`DateTime<Utc>` | Convert to and from RFC 3339 text at the query boundary
JSON object or array | `serde_json` to and from text at the boundary; no in-database JSON operators
Key, id or code column | `key_col`; `.text()` only for unindexed prose
Any string read | `AnyRowExt::get_text`, `get_text_opt`; never `try_get::<String>`
Any integer read | `AnyRowExt::get_int`, `get_int_opt`
Unsigned integer | Automatic
New row id | `query::insert_returning_id`
Cast to string | `Expr::col(c).cast_as(Alias::new(text_cast(db.backend)))`
Insert, ignoring a duplicate | `query::on_conflict_ignore(keys)`

```rust
use laterite_core::{bool_col, AnyRowExt};

Table::create()
    .table(Article::Table)
    .col(bool_col(Article::Published).not_null().default(0));

let published: bool = row.get_bool("published")?;
let title: String = row.get_text("title")?;
```

Ids are `bigint` auto-increment; an insert never sets one. Canonicalise a
user-facing unique key, lower-cased and trimmed, on write and on lookup.

## Mind the differences the helpers do not hide

Backend | Rule
--- | ---
SQLite | Foreign keys are enforced with `PRAGMA foreign_keys = ON`, set for you. Do not disable it.
MySQL | Use `key_col` or `.text()` for every string column. Keep a migration to one schema change.
MySQL | A unique key compares case-insensitively.
