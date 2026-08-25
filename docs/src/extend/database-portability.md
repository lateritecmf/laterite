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
`lat make:migration <description>`, which writes the skeleton with `strata`
already imported, so imports are never your concern.

## One migration per file

Each migration is a single file that does one thing, kept under a module's
`src/migrations/` directory and named `m<NNNN>_<description>.rs` (the leading `m`
lets the name start with a digit). The file declares a `pub struct Migration`
implementing the `Migration` trait: a stable `name`, an `up`, and an optional
`down`.

The directory's `mod.rs` is the manifest. It lists the files in apply order with
the `migration_set!` macro, which declares each as a submodule and generates the
module's stable `MODULE_ID` and its `migrations()` set:

```rust
laterite_core::migration_set! {
    module_id: "acme.blog",
    m0001_create_posts,
    m0002_create_comments,
}
```

The list is the module's version history. Append new entries at the end; never
reorder or rename a shipped one, since applied migrations are tracked by
`(module_id, name)`. `lat make:migration <description>` finds the next number,
writes the file, and adds it to the manifest for you.

## Portable representations

| You write | Stored as | How |
| --- | --- | --- |
| `bool` | `integer` (0/1) | `bool_col` in migrations; bound and read as `bool` |
| `DateTime<Utc>` | `text` (RFC 3339) | fixed-precision so string ordering is chronological |
| JSON object / array | `text` | `serde_json` at the boundary |
| key / id / code column | `varchar(255)` | `key_col` (MySQL cannot index `text`) |
| any string read | `String` | `AnyRowExt::get_text` (MySQL reports `text` as `BLOB`) |

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

### Strings, keys, and casts

Three MySQL quirks shape how strings are stored and read; the helpers below hide
all of them.

- **Key columns are `varchar`, not `text`.** MySQL cannot index a `text` column,
  so any column that is a primary key, unique key, foreign key, or part of an
  index must be a bounded string. Use `laterite_core::key_col(name)` (a
  `varchar(255)`) for ids, codes, tokens, and anything you index; use plain
  `.text()` only for unindexed prose.

- **Read strings with `get_text`, never `try_get::<String>`.** MySQL reports a
  `text` column as `BLOB` through `sqlx::Any`, so a plain `String` decode fails
  there (and even a cast-to-string of a `text` column comes back `BLOB`).
  `AnyRowExt::get_text(col)` (and `get_text_opt` for a nullable column) decodes as
  `String` on Postgres and SQLite and falls back to a byte read on MySQL:

  ```rust
  use laterite_core::AnyRowExt;

  let title: String = row.get_text("title")?;
  ```

- **Cast to a string with `text_cast`.** A descriptor-driven screen casts every
  selected column to a string so a value of any type reads back uniformly. MySQL
  casts to `char`, Postgres and SQLite to `text`, so name the target with
  `laterite_core::query::text_cast(backend)`:
  `Expr::col(c).cast_as(Alias::new(text_cast(db.backend)))`.

### Idempotent inserts

To insert a row and ignore a duplicate (an "insert or nothing"), use
`laterite_core::query::on_conflict_ignore(keys)` rather than
`OnConflict::columns(keys).do_nothing()`: sea-query renders MySQL's `do_nothing`
as invalid SQL, so the helper expresses the same intent in a form valid on every
backend.

### Case sensitivity of text keys

MySQL's default collation compares strings case- and trailing-space-insensitively,
while Postgres and SQLite are exact. A unique text key such as a username or email
therefore behaves differently per backend unless you normalise it. For any
user-facing key you look up or enforce uniqueness on, canonicalise it (lower-case
and trim) on both write and lookup, as the auth store does for usernames and
emails, so `Root` and `root ` resolve to one account everywhere.

### Behaviour the helpers do not hide

A few differences are inherent and worth knowing:

- **SQLite foreign keys.** SQLite enforces foreign keys only when
  `PRAGMA foreign_keys = ON`; `sqlx` sets this by default, so a foreign key rejects
  on SQLite as it does on Postgres and MySQL. Do not disable it.
- **`varchar` needs a length on MySQL.** Use `key_col` (a bounded `varchar`) or
  `.text()`; a bare `ColumnDef::string()` (unbounded `varchar`) is a MySQL error.
- **DDL is transactional only on Postgres and SQLite.** MySQL commits implicitly
  after each schema statement, so a migration that fails partway cannot be rolled
  back there. Keep each migration to one table or change where practical.

## Writing a portable query

Never hand-write SQL with `?` placeholders: Postgres expects `$1`, `$2`, so a raw
`?` query fails there. Always build through `sea-query` and the query helpers,
which render both the SQL and the placeholders for the connection's backend.

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
