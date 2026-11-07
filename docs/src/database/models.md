# Models

A model is a struct over one table. `#[derive(Model)]` reads and writes it
through `laterite_core::model`.

## Generate one

```sh
lat make:model article --field title:text,views:integer,published_at:datetime
```

```rust
// src/models/article.rs
use laterite_core::model::{DateTime, Utc};

#[derive(Debug, Clone, laterite_core::Model)]
#[model(table = "articles")]
pub struct Article {
    pub id: i64,
    pub title: String,
    pub views: i64,
    pub published_at: Option<DateTime<Utc>>,
}
```

## Declare one

Attribute | On | Does
--- | --- | ---
`#[model(table = "articles")]` | The struct | The table; required
`#[model(id = "article_id")]` | The struct | The id field; default `id`, an `i64`
`#[model(column = "published_at")]` | A field | The column; default the field's name

Field type | Column
--- | ---
`String` | Text; null reads as empty
`i64`, `i32` | Integer
`bool` | `bool_col`
`f64` | Floating point
`DateTime<Utc>` | UTC text
`serde_json::Value` | JSON as text
`Option<T>` | Any of the above, nullable

## Read and write

```rust
use laterite_core::model;

let id = model::insert(&db, &Article { id: 0, title: "Hello".into(), views: 0, published_at: None }).await?;
let mut article = model::find::<Article>(&db, id).await?.expect("just inserted");
article.views += 1;
model::update(&db, &article).await?;
let every = model::all::<Article>(&db).await?;
```

Function | Returns
--- | ---
`find::<M>(db, id)` | `Option<M>`
`all::<M>(db)` | Every row, in id order
`find_many::<M>(db, &ids)` | `HashMap<i64, M>`, one query
`insert(db, &model)` | The new id; the model's own `id` is ignored
`update(db, &model)` | Writes every column to the row with the model's id

## Load relations for a page

```rust
let ids: Vec<i64> = articles.iter().map(Model::id).collect();
let comments = model::has_many::<Comment>(&db, "article_id", &ids, |c| c.article_id).await?;
let authors = model::find_many::<Author>(&db, &author_ids).await?;
```

Each call is one query for the whole page: `has_many` groups the rows by their
parent id, `find_many` keys them by id.
