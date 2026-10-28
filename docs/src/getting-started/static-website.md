# Static Website

`lat new` scaffolds a static website as a theme folder: pages, layouts,
partials, content and assets, with no database and no Rust project.

```bash
lat new "Acme" --static
cd acme
lat theme serve --from themes/acme    # http://127.0.0.1:8080
lat theme build --from themes/acme    # dist/
```

`lat new` without `--static` asks first:

Prompt | Value
--- | ---
**What are you building?** | An application with an admin panel, or a static website.
**Application name** | Any text; its slug names the folder and the theme.
**Site URL** | The origin canonical links use. Default `https://<slug>.example`.

## See what it generates

```text
acme/
├── config/default.toml        # [app] name, url
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

A page carries its settings above a `==` line:

```text
url: /about
layout: default
title: About
description: Who we are and what we do.
==
<h1>{{ this.page.title }}</h1>
<p>{{ this.page.description }}</p>
```

The settings and the template language are those of a theme in an
application: [Pages](../themes/pages.md), [Layouts and Partials](../themes/layouts-and-partials.md),
[Content](../themes/content.md), [Templating](../markup/templating.md).

## Preview

```bash
lat theme serve --from themes/acme
lat theme serve --from themes/acme --listen 127.0.0.1:3000
```

Every request rebuilds the site when a file under the folder changed; a
template that does not compile shows its error in the browser.

## Build and deploy

```bash
lat theme build --from themes/acme
lat theme build --from themes/acme --out public --base-url https://acme.example
lat theme build --from themes/acme --copy docs/book:docs
```

Flag | Default | Meaning
--- | --- | ---
`--from <DIR>` | | The theme folder; its name is the theme's code.
`--out <DIR>` | `dist` | The output directory.
`--base-url <URL>` | `app.url` from `config/default.toml` | The origin canonical links use.
`--copy <SRC>:<DEST>` | none | A directory copied into the output at `<DEST>`. Repeatable.

`build.sh` runs the same build on a host that starts from nothing:

```bash
cargo install laterite-cli --locked
lat theme build --from themes/acme --out dist
```

`netlify.toml` points Netlify at it and serves `dist/`.

## Move to an application

```bash
cd themes && zip -r ../acme.zip acme && cd ..
lat new "Acme"
cd acme && lat theme import acme ../acme.zip
```

The folder is the theme's export layout: the same files become the
application's theme. See [Export and Import](../themes/export-and-import.md).
