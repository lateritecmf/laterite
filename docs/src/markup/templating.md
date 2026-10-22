# Templating

A template extends a layout, fills its blocks, includes partials and content, and reads the `theme` global.

```jinja
{% extends "layouts/default.htm" %}
{% block body %}
  {% include "partials/nav.htm" %}
  {{ partial("card.htm", title="Acme", body=page.body) }}
  {{ content("intro.md") }}
  <h1>{{ theme.name }}</h1>
{% endblock %}
```

Templates are named by the kind's folder and the path:

Kind | Name
--- | ---
`Kind::Layout` | `layouts/default.htm`
`Kind::Page` | `pages/home.htm`
`Kind::Partial` | `partials/nav.htm`
`Kind::Asset` | `assets/css/site.css`

A theme's own template is used, else the nearest ancestor's.

Name | Gives
--- | ---
`{% extends "layouts/<path>" %}`, `{% block %}` | A page inside a layout (see [Tags](tags.md)).
`{% include "partials/<path>" %}` | A partial with the current variables.
`partial(<path>, name=value, ...)`, `content(<path>, name=value, ...)` | A partial or a content block (see [Functions](functions.md)).
`theme` | `code`, `name`, `author`, `homepage`, `description` of the theme rendered, and its [settings](../themes/theme-settings.md) (see [Properties](properties.md)).

## Escape output

HTML is escaped in every template but an asset. A printed undefined value is an error; `{% if page.sub %}` on an undefined value is not.

## Check a template

```rust
use lateritecmf::web::theme::{check, RenderError};

match check(source) {
    Ok(()) => {}
    Err(RenderError { line, message, .. }) => println!("line {line:?}: {message}"),
}
```

`RenderError` carries `template`, `line`, `kind` and `message`.

## Render from Rust

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

## Add markup from a module

```rust
use lateritecmf::web::theme::markup::{Markup, MarkupReg};
use minijinja::Environment;
use std::sync::Arc;

struct Money;

impl Markup for Money {
    fn install(&self, env: &mut Environment<'static>) {
        env.add_filter("money", |minor: i64| format!("{}.{:02}", minor / 100, minor % 100));
        env.add_function("year", || 2026);
    }
}

fn register(&self, registry: &mut Registry) {
    registry.add(MarkupReg::new("acme.money", Arc::new(Money)));
}
```

```jinja
{{ price|money }} in {{ year() }}
```

Every registration is installed in order. A [static build](../themes/static-build.md#build-from-the-application) takes the registrations through `build::build_with`.
