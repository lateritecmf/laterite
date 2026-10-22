# Layouts and Partials

A layout is the frame a page renders inside; a partial is a fragment a layout or page includes.

```jinja
{# layouts/default.htm #}
<!doctype html>
<html>
<head>{{ head }}</head>
<body>
  {% include "partials/nav.htm" %}
  <main>{% block page %}{% endblock %}</main>
</body>
</html>
```

```jinja
{# pages/home.htm, with the settings {"url": "/", "layout": "default"} #}
{% block title %}{{ this.page.title }}{% endblock %}
<h1>{{ this.page.title }}</h1>
{{ partial("card.htm", title="Acme", body=page.body) }}
```

Name | Gives
--- | ---
`layout` in a page's settings | The layout by name (`default` for `layouts/default.htm`); the page then fills its `page` block, and a block the page defines at its top level fills the layout's block of that name without rendering in place. A page that writes `{% extends %}` itself is left as written.
`{% block page %}` in a layout | Where the page renders.
`{% extends "layouts/<path>" %}`, `{% block %}` | A page inside a layout.
`{% include "partials/<path>" %}` | A partial with the current variables.
`partial(<path>, name=value, ...)` | A partial with the named variables, the request's `this` and `head`, and the globals.

## Name a template

Kind | Name
--- | ---
`Kind::Layout` | `layouts/default.htm`
`Kind::Page` | `pages/home.htm`
`Kind::Partial` | `partials/nav.htm`
`Kind::Asset` | `assets/css/site.css`

A theme's own template is used, else the nearest ancestor's (see [Child Themes](child-themes.md)).

## Save a layout or partial from Rust

```rust
store::save_template(&db, site.id, Kind::Layout, "default.htm", None, LAYOUT, None).await?;
store::save_template(&db, site.id, Kind::Partial, "nav.htm", None, NAV, None).await?;
```

The path rules and the other calls are under [Themes](themes.md#save-a-template).

## Give a partial a role

Partial at | Role
--- | ---
`partials/<alias>/default.htm` | The markup of the component attached under `<alias>` (see [Components](components.md)).
`partials/<code>/default.htm` | The markup of the component offered as the snippet `<code>` (see [Snippets](snippets.md)).
A partial with `snippet` in its settings | An insertable block in the rich and markdown editors (see [Snippets](snippets.md)).
