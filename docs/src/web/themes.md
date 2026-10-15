# Themes

A theme is a set of templates stored in the database and edited from the admin. Register the module and its tables migrate with the application's:

```rust
use lateritecmf::prelude::*;
use lateritecmf::web::theme::ThemeModule;

Bootstrap::new("config")
    .module(ThemeModule)
    .module(migrations::AppModule)
    .serve()
    .await
```

Create a theme from its YAML config, add templates, and make it the site's theme:

```rust
use lateritecmf::web::theme::{self, store, Kind};

let site = store::create_theme(&db, "acme", "name: Acme\nauthor: Acme\n").await?;
store::save_template(&db, site.id, Kind::Layout, "default.htm", None, LAYOUT, None).await?;
store::save_template(
    &db,
    site.id,
    Kind::Page,
    "home.htm",
    Some(serde_json::json!({ "url": "/", "layout": "default" })),
    HOME,
    Some(operator_id),
)
.await?;
theme::set_active_theme(&db, "acme").await?;
```

## The config

```yaml
name: Acme
author: Acme
homepage: https://acme.example
description: The Acme site.
parent: base
require:
  - acme.blog
form:
  fields:
    site_name:
      label: Site name
      default: Acme
```

Key | Type | Default | Meaning
--- | --- | --- | ---
`name` | string | `""` | The theme's display name.
`author` | string | `""` | Who made it.
`homepage` | string | `""` | The author's site.
`description` | string | `""` | One line about the theme.
`parent` | string | none | The theme whose templates fill in for the ones this theme lacks.
`require` | list of module ids | `[]` | The modules the theme's templates need.
`form` | form descriptor | none | The theme's own settings fields (see [Descriptor Files](../extend/descriptor-files.md)).

A key outside this table is refused. `ThemeConfig::parse` reads the YAML and `ThemeConfig::to_yaml` writes it.

## Templates

Kind | Stored as | Holds
--- | --- | ---
`Kind::Layout` | `layout` | The frame a page renders inside.
`Kind::Page` | `page` | A page served at its URL.
`Kind::Partial` | `partial` | A fragment a layout or page includes.
`Kind::Content` | `content` | A block of HTML, Markdown or text.
`Kind::Asset` | `asset` | A text file the site serves (a stylesheet, a script).

A path is `/`-separated segments of lowercase letters, digits, dots, hyphens and underscores, at most 255 characters. `settings` is a JSON object; `None` keeps the current one.

```rust
// A theme's own templates, or those of one kind.
let all = store::list_templates(&db, site.id, None).await?;
let pages = store::list_templates(&db, site.id, Some(Kind::Page)).await?;

// Through the parent chain: the theme's own row, else the nearest ancestor's.
if let Some((from, template)) = store::resolve_template(&db, "child", Kind::Layout, "default.htm").await? {
    println!("{} from {}", template.path, from.code);
}

store::delete_template(&db, template.id).await?;
```

## Revisions

Every save keeps what it replaced:

```rust
let revisions = store::revisions(&db, template.id).await?; // newest first
let restored = store::restore_revision(&db, revisions[0].id, Some(operator_id)).await?;
```

## Themes

```rust
let all = store::list_themes(&db).await?;
let one = store::find_theme(&db, "acme").await?;
store::update_theme_config(&db, "acme", "name: Acme\nparent: base\n").await?;
store::set_locked(&db, "acme", true).await?;
let copy = store::duplicate_theme(&db, "acme", "acme-copy").await?;
let child = store::create_child_theme(&db, "acme", "acme-child", "Acme child").await?;
store::delete_theme(&db, "acme-copy").await?;
```

Call | Refused when
--- | ---
`create_theme` | The code is taken, is not lowercase letters, digits and hyphens, or the parent is missing or leads back to the theme.
`update_theme_config` | The theme is locked, or the parent is missing or leads back.
`save_template`, `delete_template` | The theme is locked, or the path is not a template path.
`delete_theme` | The theme is active, locked, or another theme's parent.

## Rendering

```rust
use lateritecmf::web::theme::{Engine, EngineCache, Kind};

let engine = Engine::build(&db, "acme").await?;
let html = engine.render(Kind::Page, "home.htm", serde_json::json!({ "title": "Home" }))?;
let css = engine.render(Kind::Asset, "css/site.css", ())?;

// One engine per theme, rebuilt when the theme or an ancestor changes.
let cache = EngineCache::new();
let engine = cache.get(&db, "acme").await?;
cache.invalidate("acme");

// The application's own functions, filters and globals.
let engine = Engine::build_with(&db, "acme", |env| {
    env.add_function("year", || 2026);
})
.await?;
```

Templates are named by the kind's folder and the path:

Kind | Name
--- | ---
`Kind::Layout` | `layouts/default.htm`
`Kind::Page` | `pages/home.htm`
`Kind::Partial` | `partials/nav.htm`
`Kind::Asset` | `assets/css/site.css`

A theme's own template is used, else the nearest ancestor's. In a template:

```jinja
{% extends "layouts/default.htm" %}
{% block body %}
  {% include "partials/nav.htm" %}
  {{ partial("card.htm", title="Acme", body=page.body) }}
  {{ content("intro.md") }}
  <h1>{{ theme.name }}</h1>
{% endblock %}
```

Name | Gives
--- | ---
`{% extends "layouts/<path>" %}`, `{% block %}` | A page inside a layout.
`{% include "partials/<path>" %}` | A partial with the current variables.
`partial(<path>, name=value, ...)` | A partial with the named variables and the globals only.
`content(<path>)` | A content block: `.md` rendered to HTML, `.htm` and `.html` as they are, anything else escaped as text.
`theme` | `code`, `name`, `author`, `homepage`, `description` of the theme rendered.

HTML is escaped in every template but an asset. A printed undefined value is an error; `{% if page.sub %}` on an undefined value is not. A render spends at most five million instructions.

```rust
use lateritecmf::web::theme::{check, RenderError};

match check(source) {
    Ok(()) => {}
    Err(RenderError { line, message, .. }) => println!("line {line:?}: {message}"),
}
```

`RenderError` carries `template`, `line`, `kind` and `message`.

## The active theme and the edit theme

```rust
theme::set_active_theme(&db, "acme").await?;
let live = theme::active_theme(&db).await?;

theme::set_edit_theme(&db, operator_id, Some("acme-next")).await?;
let editing = theme::edit_theme(&db, operator_id).await?; // the operator's choice, else the active theme
theme::set_edit_theme(&db, operator_id, None).await?;
```

`ThemeSettings` (code `laterite.theme`) holds the active theme; `EditTheme` (code `laterite.theme.edit`) is each operator's preference.
