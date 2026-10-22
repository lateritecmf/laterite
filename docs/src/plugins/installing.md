# Installing Plugins

A plugin is a crate compiled into the application. Installing one records it
and rebuilds.

State | Where | Changing it costs
--- | --- | ---
Which plugins are compiled in | `plugins/plugins.toml` | a rebuild
Which of them are enabled | the database | a restart

## Add one

```console
$ lat plugin add ../acme-blog-plugin
$ lat plugin add https://github.com/acme/blog-plugin
$ cargo build
```

A checkout outside the project is linked into `plugins/`, not copied. `--as
<folder>` picks the folder name.

## Edit the list

```toml
# plugins/plugins.toml
[[plugin]]
path = "acme-blog"

[[plugin]]
path = "acme-blog"
source = "https://github.com/acme/blog-plugin"
```

Command | Description
--- | ---
`lat plugin add` | Records a plugin and syncs.
`lat plugin remove` | Drops the entry. `--delete` removes the folder; a link only loses its link.
`lat plugin sync` | Regenerates `plugins-manifest/` from the list. Run after editing by hand.
`lat plugin list` | Prints the plugins the application compiles in.
`lat plugin check` | Checks each installed plugin against the expected layout.
`lat doctor` | Checks the list and the manifest agree.

Two entries resolving to one crate are refused.

## Enable and disable

**Settings → Plugins** turns a compiled-in plugin off without a rebuild. The
change applies at the next start: a disabled plugin's migrations do not run, it
contributes nothing, and anything depending on it is skipped. Its tables stay.
