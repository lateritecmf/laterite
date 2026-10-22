# Queries

A query is built with the query builder and bound onto the pool through the
portable helpers; the same code runs on Postgres, MySQL and SQLite.

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

Helper | Description
--- | ---
`build(backend, &stmt)` | Renders a statement to `(sql, values)` for the backend.
`bind_values(query, values)` | Binds the values onto a query, in order. A `bool` binds as a `bool`.
`bind_values_as(query_as, values)` | The same, for a `query_as` mapping rows into a type.
`insert_returning_id(db, stmt, id_column)` | Runs an insert and returns the new auto-increment id.
`insert_returning_id_on(backend, conn, stmt, id_column)` | The same, on a connection inside a transaction.
`on_conflict_ignore(keys)` | An insert's conflict clause that ignores a duplicate of `keys`.
`text_cast(backend)` | The type name to cast a column to a string: `Expr::col(c).cast_as(Alias::new(text_cast(db.backend)))`.

## Insert a row

```rust
let stmt = Query::insert()
    .into_table(Article::Table)
    .columns([Article::Title, Article::Published])
    .values_panic(["Hello".into(), true.into()])
    .to_owned();
let id = insert_returning_id(&db, stmt, Article::Id).await?;
```

## Read a row

```rust
use laterite_core::AnyRowExt;

let published: bool = row.get_bool("published")?;
let title: String = row.get_text("title")?;
let summary: Option<String> = row.get_text_opt("summary")?;
let views: i64 = row.get_int("views")?;
```

`AnyRowExt` | Description
--- | ---
`get_bool(column)` | A boolean.
`get_text(column)`, `get_text_opt(column)` | A string, or a nullable one. Never `try_get::<String>`.
`get_int(column)`, `get_int_opt(column)` | An integer of any declared width as `i64`, or a nullable one.
