# Installing Plugins

A plugin is a separate crate that contributes migrations, admin screens, settings
and routes. Rust links at build time, so a plugin is **compiled into** your
application: installing one means recording it and rebuilding, not dropping a
folder somewhere and restarting.

That splits plugin state in two, and each half is kept where it can be honoured:

| | Where it lives | Changing it costs |
|---|---|---|
| Which plugins are compiled in | `plugins/plugins.toml` | a rebuild |
| Which of them are enabled | the database | a restart |

## Adding one

From a local checkout:

```console
$ lat plugin add ../rainmill-discovery
Linked plugins/rainmill-discovery -> /home/you/code/rainmill-discovery
Synced 1 plugin(s) into plugins-manifest/:
  rainmill-discovery         plugins/rainmill-discovery

Added rainmill-discovery at plugins/rainmill-discovery.
```

Or from a repository:

```console
$ lat plugin add https://github.com/rainmill/discovery-plugin
```

Then `cargo build`. A plugin outside your project is linked into `plugins/`
rather than copied, so it stays where it is and your edits apply in place. One
inside `plugins/` already is simply recorded where it sits.

**Folder names carry no meaning.** The list says where each plugin is, and the
plugin's own `Cargo.toml` says what it is called, so nothing has to be renamed
to be installed. Use `--as <folder>` if you want a particular name.

## The list

```toml
# plugins/plugins.toml
[[plugin]]
path = "rainmill-discovery"

[[plugin]]
path = "acme-blog"
source = "https://github.com/acme/blog-plugin"
```

`lat plugin add` and `lat plugin remove` maintain it; `lat plugin sync`
regenerates the `plugins-manifest` crate from it. Editing it by hand is fine,
as long as you run `sync` afterwards. `lat doctor` checks the two agree.

A plugin can only be compiled in once, so two entries resolving to the same
crate are refused.

## Removing one

```console
$ lat plugin remove rainmill-discovery
```

The folder is left alone unless you pass `--delete`, and a linked plugin only
ever loses its link: the checkout it points at is your working copy, not the
command's to delete.

## Enabling and disabling

This is the part that needs no rebuild. Every compiled-in plugin is recorded in
the database at boot and listed under **Settings → Plugins**, where an operator
can turn one off. The change is an intent stored in the database and applied on
the next boot, so a disabled plugin's migrations do not run and it contributes
nothing. Anything that depends on it is skipped with it.

Disabling leaves a plugin's tables in place, so turning it back on does not lose
data.

## Writing one

A plugin crate declares itself in its manifest:

```toml
[package.metadata.laterite]
plugin = "acme.blog"
```

That marks the crate as a plugin and names the module it registers. A tool can
read it without building anything, which is how `lat plugin add` refuses a crate
that is not a plugin, and how a marketplace lists one.

There is deliberately **no version field** there. Which Laterite a plugin supports
is stated by its dependency requirement, below, and stating it twice would give
you two things to keep in step.

A plugin crate exposes one entry point at its root:

```rust
pub fn module() -> Box<dyn Module> {
    Box::new(MyModule)
}
```

The generated manifest calls `<crate>::module()` by name, so it must be at the
crate root. A `module()` hidden behind a feature or nested in a submodule
compiles cleanly and registers nothing.

Declare the framework by version, never by path:

```toml
[dependencies]
laterite-core = "0.6"
laterite-admin = "0.6"
```

That requirement is also the compatibility statement. `lat plugin add` reads it
before fetching or building and refuses a plugin built for a Laterite you are not
on, naming both versions:

```console
$ lat plugin add ../acme-legacy
Error: acme-legacy needs laterite-core ^0.2, and this application is on 0.5.0.
Look for a release of the plugin that supports 0.5, or upgrade this
application to one it supports.
```

Because the framework crates release together, a requirement on any one of them
states the generation. A plugin that names none (one developed against a local
checkout) is not second-guessed.

That keeps the plugin's manifest independent of where it sits on disk. An
application building against a local checkout of the framework points those at
it with `[patch.crates-io]`, which `lat new` sets up in development mode. Without
it the plugin would link a second copy of the framework from crates.io, and its
`Module` would be a different type from the one your application expects.
