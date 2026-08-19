# Database Portability

Laterite runs on **Postgres, MySQL, or SQLite**, chosen by the connection URL in
configuration. The data layer is built on `sea-query` (which renders SQL for the
running backend) over `sqlx::Any` (which dials any of the three at runtime), so a
migration or query is written once and works everywhere.

`sqlx::Any` speaks a common subset of column types across backends, and a few
native types do not travel cleanly (a SQLite `boolean` column, Postgres arrays,
or `jsonb`). Rather than make you learn those quirks, the framework
**polyfills** them: you write natural Rust types, and the framework stores a
portable representation and converts at the boundary. When you hit a new quirk,
add a polyfill in the same spirit rather than pushing it onto callers.

## The `strata` toolkit

Migrations stack the schema in layers, like rock strata, and `strata` is the
one import that gathers everything a layer needs. You should not have to import
each helper or remember which polyfill a type takes. `use laterite_core::strata::*;`
brings in the whole migration and query toolkit at once, the `Migration` trait
and its `async_trait` macro, the schema and query builders, the `Db` handle, and
the polyfills (`bool_col`, `AnyRowExt`). A migration file starts with that single
line. Better still, generate the file with the scaffolding command,
`lat make migration <name>`, which writes the skeleton with `strata` already
imported, so imports are never your concern.

## Portable representations

| You write | Stored as | How |
| --- | --- | --- |
| `bool` | `integer` (0/1) | `bool_col` in migrations; bound and read as `bool` |
| `DateTime<Utc>` | `text` (RFC 3339) | fixed-precision so string ordering is chronological |
| JSON object / array | `text` | `serde_json` at the boundary |

`sqlx::Any` has no unsigned column types, so the bind layer also maps unsigned
integers to the next wider signed integer. You rarely write these yourself, but
the query builder emits them for `LIMIT` and `OFFSET`, so pagination stays
portable without any handling on your part.

### Booleans

A native `boolean` column is not decodable through `sqlx::Any` on SQLite, so
booleans are stored as `0`/`1` integers on every backend. The framework hides
this in three places, so you keep using `bool`:

- **Schema**: use `laterite_core::bool_col` instead of `ColumnDef::new(..).boolean()`.

  ```rust
  use laterite_core::bool_col;

  Table::create()
      .table(Article::Table)
      .col(bool_col(Article::Published).not_null().default(0))
      // ...
  ```

- **Bind**: a `bool` value binds as an integer automatically, so filters read
  naturally: `.and_where(Expr::col(Article::Published).eq(true))`.

- **Read**: `AnyRowExt::get_bool` reads it back:

  ```rust
  use laterite_core::AnyRowExt;

  let published: bool = row.get_bool("published")?;
  ```

### Ids and timestamps

Ids are `bigint` auto-increment primary keys the database assigns, so an insert
never sets the id. Reading the new id back is not uniform (Postgres and SQLite
support `RETURNING`; MySQL reports a last-insert-id), so use
`laterite_core::query::insert_returning_id`, which handles both and hands you an
`i64`. Timestamps are RFC 3339 `text` written at a fixed precision, so a
`WHERE expires_at > ?` comparison orders correctly as a string, and convert to
`DateTime<Utc>` at the query boundary.

### JSON

Store an object or array as `text` and (de)serialize with `serde_json`. Only
whole-value read/write is portable; do not rely on in-database JSON operators or
indexes, which are Postgres-specific.

## Writing a portable query

Build the statement with `sea-query` and run it through the query helpers, which
render for the connection's backend and bind values portably:

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

`db` here is a `laterite_core::Db`: the connection pool paired with its backend.
Passing it (rather than a bare pool) is what lets the query layer render SQL for
the right database.
