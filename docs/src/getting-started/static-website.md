# Static Website

`lat new` scaffolds a static website as a theme folder: pages, layouts,
partials, content and assets, with no database and no Rust project.

```bash
lat new "Acme" --static
cd acme
lat serve          # http://127.0.0.1:8080, rebuilt on every change
lat theme build    # dist/
```

`lat new` without `--static` asks first:

Prompt | Value
--- | ---
**What are you building?** | An application with an admin panel, or a static website.
**Where does the site's content live?** | In this folder as files you edit (`--content files`, what `--static` alone means); in this folder, edited in the admin (`folder`); in this folder, committed by the admin (`git`); in a database with the admin beside it (`database`).
**Application name** | Any text; its slug names the folder and the theme.
**Site URL** | The origin canonical links use.
**Database**, **Build hook URL** | For `database`: the connection, and the host's hook (blank to set later).
**First administrator** | For every choice but `files`: username, email, password.

A site whose content is edited in the admin is an application whose build is static: it carries the Cargo project, `src/main.rs` with the theme module, the framework's own tables in `storage/site.db` (or the database named), and the theme folder. `lat serve` runs its admin; `lat theme build` renders `dist/`; `lat theme publish` pushes (`git`) or calls the build hook (`database`). Default `https://<slug>.example`.

## See what it generates

```text
acme/
├── config/default.toml        # [app] name, url; [theme] folder, copy
├── themes/acme/
│   ├── theme.yaml
│   ├── layouts/default.htm
│   ├── pages/home.htm         # url: /
│   ├── pages/about.htm        # url: /about
│   ├── pages/404.htm
│   ├── partials/nav.htm
│   ├── content/intro.md
│   └── assets/css/site.css
├── build.sh                   # installs lat, runs the build
├── netlify.toml
└── README.md
```

A page carries its settings as front matter above the markup:

```text
---
url: /about
layout: default
title: About
description: Who we are and what we do.
---
<h1>{{ this.page.title }}</h1>
<p>{{ this.page.description }}</p>
```

The settings and the template language are those of a theme in an
application: [Pages](../themes/pages.md), [Layouts and Partials](../themes/layouts-and-partials.md),
[Content](../themes/content.md), [Templating](../markup/templating.md).

## Set the config

```toml
[app]
name = "Acme"
url = "https://acme.example"

[server]
listen = "127.0.0.1:8080"

[theme]
folder = "themes/acme"
copy = ["static:static"]
```

Key | Default | Meaning
--- | --- | ---
`app.url` | | The origin canonical links use.
`server.listen` | `127.0.0.1:8080` | The preview's address.
`theme.folder` | | The theme folder, relative to the project; its name is the theme's code.
`theme.copy` | `[]` | Directories copied into the output, `<src>:<dest>`, relative to the project. A source absent on this machine is skipped with a note; a `--copy` flag's is required.

## Preview

```bash
lat serve
lat serve --port 3000
lat theme serve --from themes/other
```

Every request rebuilds the site when a file under the folder changed; a
template that does not compile shows its error in the browser.

## Build and deploy

```bash
lat theme build
lat theme build --out public --base-url https://acme.example
lat theme build --from themes/other --copy docs/book:docs
```

Flag | Default | Meaning
--- | --- | ---
`--from <DIR>` | `theme.folder` | The theme folder.
`--out <DIR>` | `dist` | The output directory.
`--base-url <URL>` | `app.url` | The origin canonical links use.
`--copy <SRC>:<DEST>` | `theme.copy` | A directory copied into the output at `<DEST>`. Repeatable, added to the config's.

`build.sh` runs the same build on a host that starts from nothing:

```bash
cargo install laterite-cli --locked
lat theme build --out dist
```

`netlify.toml` points Netlify at it and serves `dist/`.

## Choose where the content lives

```toml
[theme]
folder = "themes/acme"
source = "folder"
```

Source | The theme is | Edited | Published
--- | --- | --- | ---
`database` | the rows in `[database]` | in the admin | `lat theme publish` calls `theme.publish.hook`
`folder` | the theme folder | in your editor, or in the admin, which writes the files | commit the folder
`git` | the theme folder | in the admin, which commits every save | `lat theme publish` pushes
a module's | its own | its own | its own

`folder` and `database` are always available; `git` needs the `theme-git` feature:

```toml
lateritecmf = { version = "0.9", features = ["sqlite", "theme-git"] }
```

```toml
[theme]
folder = "themes/acme"
source = "git"

[theme.git]
remote = "origin"
branch = "main"

[theme.publish]
hook = "https://api.netlify.com/build_hooks/..."
```

Key | Default | Meaning
--- | --- | ---
`theme.source` | `database` | The source by code.
`theme.git.remote`, `theme.git.branch` | `origin`, `main` | Where `publish` pushes.
`theme.publish.hook` | none | A URL `lat theme publish` posts to after the source published; the host rebuilds on it.

A file-backed source is read in before every build, and every save in the admin is written back. The framework's own tables (users, sessions, settings, logs) stay in `[database]`.

## Edit from a laptop

```bash
cargo build --release
./target/release/acme admin
```

`admin` serves the application and opens the admin in the browser. With a `folder` or `git` source and [records in files](../database/records-in-files.md), every save lands in the checked-out repository; push it to publish.

Command | Does
--- | ---
`acme` or `acme serve` | Serves the admin and the site on `server.listen`.
`acme admin` | The same, and opens `/admin` in the browser.
`acme build` | Renders `dist/`.
`acme publish` | Pushes, or calls the build hook.

## Add a plugin

```bash
lat plugin add https://github.com/rainmill/discovery-plugin
lat new "Acme" --static --plugin ../discovery-plugin
```

The first plugin gives the site a Cargo project (`Cargo.toml`, `src/main.rs`,
`plugins/`); `lat serve` and `lat theme build` run that binary from then on,
and the plugin takes part in the build. A plugin a static website can use
declares it in its manifest:

```toml
[package.metadata.laterite]
plugin = "rainmill.discovery"
static = true
```

A plugin without `static = true` is refused at install; one whose module
answers `supports_static` false stops the build, by name. A plugin's
definitions live under `plugins/<module id>/`:

```text
acme/
├── plugins/
│   ├── plugins.toml
│   └── rainmill.discovery/
│       ├── robots.yaml
│       └── sitemap.yaml
├── src/main.rs
└── Cargo.toml
```

```yaml
# plugins/rainmill.discovery/robots.yaml
groups:
  - agents: ["*"]
    disallow: ["/drafts/"]
```

What each plugin reads is on its own page; the discovery plugin writes
`robots.txt` and a `sitemap.xml` of every page the build wrote.

## Move to an application

```bash
cd themes && zip -r ../acme.zip acme && cd ..
lat new "Acme"
cd acme && lat theme import acme ../acme.zip
```

The folder is the theme's export layout: the same files become the
application's theme. See [Export and Import](../themes/export-and-import.md).
