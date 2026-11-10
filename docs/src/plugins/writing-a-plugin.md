# Writing a Plugin

`lat make:plugin` scaffolds a plugin crate; `lat plugin add` installs it.

```bash
lat make:plugin acme.blog            # blog-plugin/: Cargo.toml, src/lib.rs
lat make:plugin acme.blog --admin    # adds src/admin/
lat plugin add ./blog-plugin
```

Argument | Description
--- | ---
`<ID>` | The module id, `vendor.name`. The crate is `vendor-name`.
`--path <PATH>` | Where to create it. Default the current directory.
`--admin` | Scaffold the admin half: descriptors, screens, settings.

## Mark the crate

Name its module, and declare the framework by version:

```toml
[package.metadata.laterite]
plugin = "acme.blog"

[dependencies]
laterite-core = "0.10"
laterite-admin = "0.10"
```

The dependency requirement is the compatibility statement. `lat plugin add`
refuses a plugin built for another Laterite:

```console
$ lat plugin add ../acme-legacy
Error: acme-legacy needs laterite-core ^0.2, and this application is on 0.5.0.
```

Never declare the framework by path. An application on a local checkout maps
it with `[patch.crates-io]`, which `lat new` writes in development mode.

## Use the framework's crates

```rust
use laterite_core::sea_query::{self, Alias, Expr, Iden, Query};
use laterite_core::serde_json::json;
use laterite_core::{minijinja, sqlx};
use laterite_admin::axum::Router;
```

Crate | Reach it at
--- | ---
`sqlx` | `laterite_core::sqlx`
`sea-query` | `laterite_core::sea_query` (with `use`, `#[derive(Iden)]` works)
`serde_json` | `laterite_core::serde_json`
`minijinja` | `laterite_core::minijinja`
`axum` | `laterite_admin::axum`
`serde` | a dependency of the plugin's own: `serde = { version = "1", features = ["derive"] }`

`lat plugin sync`, `lat plugin check` and `lat doctor` read the application's dependency graph:

```console
$ lat plugin sync
Dependencies:
  error sea-query is built in 2 versions (0.31.1 for acme-maps; 0.32.7 for the framework); values of one do not fit the other, so these cannot exchange them. Move the plugin onto the framework's version.
  warn  acme-maps depends on sea-query itself; reach it through `laterite_core::sea_query` so a version change on either side cannot split its types
  note  44 other crate(s) are built in more than one version, none crossing a plugin boundary (`lat plugin check` lists them)
```

Finding | Level
--- | ---
A crate whose types cross the boundary (`sqlx`, `sea-query`, `serde_json`, `minijinja`, `axum`, `laterite-*`) in two versions | error
Two plugins, or a plugin and the framework, linking the same native library | error
A plugin listing a re-exported crate itself | warning
Any other crate in two versions | note

## Expose the module

At the crate root:

```rust
use laterite_core::{migration_set, BootCx, CoreResult, MigrationSet, Module, ModuleId, Registry};

migration_set! {
    module_id: "acme.blog",
}

pub fn module() -> Box<dyn Module> {
    Box::new(Plugin)
}

pub struct Plugin;

impl Module for Plugin {
    fn id(&self) -> ModuleId {
        ModuleId::new("acme.blog")
    }
    fn migrations(&self) -> MigrationSet {
        migrations()
    }
    fn register(&self, registry: &mut Registry) {
        // registry.add_resource(..);
        // registry.add_permission(..);
        // registry.add_settings(..);
    }
    async fn boot(&self, cx: &BootCx<'_>) -> CoreResult<()> {
        // A sweep, a seed or a check, with cx.db and cx.registry.
        Ok(())
    }
}
```

Method | Runs | Description
--- | --- | ---
`migrations` | Before `register`, in dependency order. | Applies the module's own migrations. See [Migrations](../database/migrations.md).
`register` | Once every module has migrated. | Contributes resources, [settings models](../admin/settings.md), [permissions](../admin/permissions.md), menu entries, hooks.
`boot` | Once every module has registered, before anything is served. | Anything that needs the database and the whole registry. An error quarantines the plugin.

## Add a resource

```bash
lat make:resource article --field title:text --field body:text --field published:boolean
```

Scaffolds the entity's migration, schema, store and admin screen, mounted at
`/articles`. Field types: `text`, `integer`, `boolean`, `datetime`. See
[Resource Features and Hooks](resource-hooks.md). `--source content/articles`
keeps the records as files; see [Records in Files](../database/records-in-files.md).
