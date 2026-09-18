# Installing Plugins

A plugin is a crate compiled into the application. Installing one records it
and rebuilds.

State | Where | Changing it costs
--- | --- | ---
Which plugins are compiled in | `plugins/plugins.toml` | a rebuild
Which of them are enabled | the database | a restart

## Add one

```console
$ lat plugin add ../rainmill-discovery
$ lat plugin add https://github.com/rainmill/discovery-plugin
$ cargo build
```

A checkout outside the project is linked into `plugins/`, not copied. `--as
<folder>` picks the folder name.

## The list

```toml
# plugins/plugins.toml
[[plugin]]
path = "rainmill-discovery"

[[plugin]]
path = "acme-blog"
source = "https://github.com/acme/blog-plugin"
```

Command | Description
--- | ---
`lat plugin add` | Records a plugin and syncs.
`lat plugin remove` | Drops the entry. `--delete` removes the folder; a link only loses its link.
`lat plugin sync` | Regenerates `plugins-manifest/` from the list. Run after editing by hand.
`lat doctor` | Checks the list and the manifest agree.

Two entries resolving to one crate are refused.

## Enable and disable

**Settings → Plugins** turns a compiled-in plugin off without a rebuild. The
change applies at the next boot: a disabled plugin's migrations do not run, it
contributes nothing, and anything depending on it is skipped. Its tables stay.

## Write one

Mark the crate, name its module, and declare the framework by version:

```toml
[package.metadata.laterite]
plugin = "acme.blog"

[dependencies]
laterite-core = "0.7"
laterite-admin = "0.7"
```

Expose the entry point at the crate root:

```rust
pub fn module() -> Box<dyn Module> {
    Box::new(MyModule)
}
```

The dependency requirement is the compatibility statement. `lat plugin add`
refuses a plugin built for another Laterite:

```console
$ lat plugin add ../acme-legacy
Error: acme-legacy needs laterite-core ^0.2, and this application is on 0.5.0.
```

Never declare the framework by path. An application on a local checkout maps
it with `[patch.crates-io]`, which `lat new` writes in development mode.
