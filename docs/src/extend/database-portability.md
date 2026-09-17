# Database Portability

Laterite runs on Postgres, MySQL or SQLite, chosen by the connection URL.
Migrations and queries are written once, through `sea-query` over `sqlx::Any`.

## Write a migration

```rust
use laterite_core::strata::*;
```

One import brings the `Migration` trait, `async_trait`, the schema and query
builders, `Db`, and the polyfills. `lat make:migration <description>` writes a
new file with it.

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

## Use portable types

You write | Stored as | Helper
--- | --- | ---
`bool` | `integer` 0/1 | `bool_col` in schema; binds as `bool`; read with `AnyRowExt::get_bool`
`DateTime<Utc>` | `text`, RFC 3339 at fixed precision | Convert at the query boundary
JSON object or array | `text` | `serde_json` at the boundary; no in-database JSON operators
Key, id or code column | `varchar(255)` | `key_col`; `.text()` only for unindexed prose
Any string read | `String` | `AnyRowExt::get_text`, `get_text_opt`; never `try_get::<String>`
Unsigned integer | The next wider signed | Automatic
New row id | `i64` | `query::insert_returning_id`
Cast to string | `char` or `text` | `Expr::col(c).cast_as(Alias::new(text_cast(db.backend)))`
Insert, ignoring a duplicate | | `query::on_conflict_ignore(keys)`

```rust
use laterite_core::{bool_col, AnyRowExt};

Table::create()
    .table(Article::Table)
    .col(bool_col(Article::Published).not_null().default(0));

let published: bool = row.get_bool("published")?;
let title: String = row.get_text("title")?;
```

Ids are `bigint` auto-increment; an insert never sets one. Canonicalise a
user-facing unique key, lower-cased and trimmed, on write and on lookup: MySQL
compares case-insensitively.

## Write a query

```rust
use laterite_core::strata::*;

let stmt = Query::select()
    .column(Article::Title)
    .from(Article::Table)
    .and_where(Expr::col(Article::Published).eq(true))
    .to_owned();
let (sql, values) = build(db.backend, &stmt);
let rows = bind_values(sqlx::query(&sql), values).fetch_all(&db.pool).await?;
```

Never hand-write SQL with `?` placeholders. `db` is a `laterite_core::Db`: the
pool paired with its backend.

## Differences the helpers do not hide

Backend | Behaviour
--- | ---
SQLite | Foreign keys are enforced only with `PRAGMA foreign_keys = ON`; `sqlx` sets it. Do not disable it.
MySQL | `varchar` needs a length: use `key_col` or `.text()`. DDL is not transactional: keep a migration to one change.
