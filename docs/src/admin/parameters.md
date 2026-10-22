# Parameters

A parameter is a JSON value a module writes for itself, under a
`namespace::group.item` key.

## Read and write one

```rust
use laterite_core::parameters;

parameters::set(ctx.db(), "acme.blog::sync.cursor", &cursor).await?;
let cursor: Option<Cursor> = parameters::get(ctx.db(), "acme.blog::sync.cursor").await?;
let removed = parameters::reset(ctx.db(), "acme.blog::sync.cursor").await?;
```

Function | Description
--- | ---
`get::<T>(db, key)` | `Ok(None)` when unset.
`get_value(db, key)` | The raw `serde_json::Value`.
`set(db, key, &value)` | Replaces any value held.
`reset(db, key)` | Removes it; `true` when one was set.

Key part | Holds
--- | ---
`namespace` | The owning module's id: `acme.blog`.
`group` | A family of items: `sync`.
`item` | The item; may contain dots.

A malformed key is `CoreError::Validation`.

## Go through the cache

```rust
let parameters = ctx.parameters();
parameters.set("acme.blog::sync.cursor", &cursor).await?;
let page: u32 = parameters.get_or("acme.blog::sync.page", 1).await?;
parameters.forget("acme.blog::sync.cursor");
```

Method | Description
--- | ---
`get`, `get_or`, `set`, `reset` | As the functions, through the cache.
`forget(key)` | The next read goes to the database.
`clear()` | Drops every entry.
`with_ttl(duration)` | On `Parameters::new(db)`; the default is five minutes.

## Read a key the framework writes

Key | Value
--- | ---
`laterite.backend::setup.completed_at` | RFC 3339, written when the first administrator is created. The constant is `laterite_admin::SETUP_COMPLETED_AT`.

## Add it to a module list

```rust
use laterite_core::parameters::ParametersModule;
```

`laterite_admin::builtin_modules()` includes it; an application composing its
own list adds `ParametersModule`.
