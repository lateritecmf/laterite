# Cache

A value is kept under a key for a lifetime, together with the namespaces it was built from. A write bumps its namespace; every value built from that namespace misses from then on, in every process of the deployment.

```rust
use std::time::Duration;
use laterite_core::cache::records;

let menu: Vec<MenuItem> = ctx
    .cache()
    .remember(
        "acme.blog::menu",
        Duration::from_secs(3600),
        &[&records("posts"), "acme.blog::feed"],
        || async { load_menu(ctx.db()).await },
    )
    .await?;
```

Method | Does
--- | ---
`remember(key, ttl, deps, load)` | The value under `key`, or what `load` returns, stored for `ttl` as JSON and built from `deps`. Concurrent callers for one key share one load.
`get(key)` | The bytes under `key` while every namespace in its `deps` stands where it stood; `get_json::<T>` decodes them.
`put(key, bytes, deps, ttl)` | Stores bytes; `put_json(key, &value, deps, ttl)` stores a value as JSON.
`add(key, bytes, deps, ttl)` | Stores only when nothing is under `key`; `true` when it did.
`forget(key)` | Drops the value under `key`.
`bump(namespace)` | Moves the namespace on; returns the new generation.
`generation(namespace)` | The namespace's generation.
`lock(name, ttl)` | `Some(lock)` when no other holder has `name`; see below.
`flush()` | Drops every value this process holds.

A namespace in `deps` is one registered below, or one of the record family `records::<entity>`. An unregistered one is refused with `CoreError::Validation`.

## Configure the store

```toml
# config/default.toml
[cache]
store = "auto"            # auto | null | memory | redis
memory_mib = 64
poll_seconds = 2
# url = "redis://127.0.0.1:6379/0"
# prefix = "acme"
```

Key | Description
--- | ---
`cache.store` | `auto` is `null` while `app.debug` is on and `memory` otherwise. Default `auto`.
`cache.memory_mib` | The `memory` store's capacity. Default `64`.
`cache.poll_seconds` | How often a process reads the generations the other processes bumped; `0` turns it off for a single process. Default `2`.
`cache.url` | The Redis URL for the `redis` store.
`cache.prefix` | The prefix of every Redis key. Default `laterite`.

```toml
[dependencies]
laterite-core = { version = "0.9", features = ["redis"] }
```

Store | Values | Generations and locks
--- | --- | ---
`null` | Kept nowhere | The database
`memory` | This process | The database
`redis` | Redis | Redis

## Register a namespace

```rust
use laterite_core::cache::CacheNamespace;

fn register(&self, registry: &mut Registry) {
    registry.add(CacheNamespace::new("feed"));
}
```

The namespace is `acme.blog::feed`: the registering module's id, two colons, the name.

Namespace | Bumped by
--- | ---
`records::<entity>` | Every save and delete of the entity through the admin or a `FormHandle`.
`laterite.settings::values` | A save through `ctx.settings()`.
`laterite.parameters::values` | A write through `ctx.parameters()`.
`laterite.theme::templates` | A theme write through `Themes`.

## Write through the handles

```rust
ctx.settings().save(&BrandSetting { app_name: "Acme".into(), ..Default::default() }).await?;
ctx.parameters().set("acme.blog::sync.cursor", &cursor).await?;
Themes::of(ctx).save_template(theme.id, Kind::Page, "about.htm", None, markup, None).await?;
ctx.cache().bump("acme.blog::feed").await?;
```

Handle | Raw function
--- | ---
`ctx.settings().set` / `.save` | `laterite_admin::settings::set` / `save`
`ctx.parameters().set` / `.reset` | `laterite_core::parameters::set` / `reset`
`Themes::of(ctx).save_template` and the other writes | `laterite_web::theme::store::*`

A raw function writes the table alone; the handle writes it and bumps.

## Hold a lock

```rust
if let Some(lock) = ctx.cache().lock("acme.blog::sync", Duration::from_secs(300)).await? {
    sync(ctx.db()).await?;
    lock.release().await?;
}
```

A lock not released lapses when its lifetime passes.

## Clear from the command line

```bash
lat cache clear                          # every namespace
lat cache clear --namespace records::posts
lat cache list
```
