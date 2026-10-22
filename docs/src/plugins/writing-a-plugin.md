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
laterite-core = "0.9"
laterite-admin = "0.9"
```

The dependency requirement is the compatibility statement. `lat plugin add`
refuses a plugin built for another Laterite:

```console
$ lat plugin add ../acme-legacy
Error: acme-legacy needs laterite-core ^0.2, and this application is on 0.5.0.
```

Never declare the framework by path. An application on a local checkout maps
it with `[patch.crates-io]`, which `lat new` writes in development mode.

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
[Resource Features and Hooks](resource-hooks.md).
