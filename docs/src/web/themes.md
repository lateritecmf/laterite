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

## The screen

`ThemeModule` registers the Themes tab at `/admin/themes` under the permission `web.manage_themes`, granted to the Administrator role. The tab lists the themes with their state, creates, duplicates, locks, activates and deletes them, and opens a theme to its templates by kind, its config and a template's editor with the earlier versions to restore.

Setting | Default | Meaning
--- | --- | ---
`backend.paths."laterite.theme/themes"` | `/themes` | Where the tab mounts under the admin, as a deployment override.

A module's own screen audits through the route context:

```rust
ctx.audit(&user, "acme.thing.save", Some("thing"), Some(&id), Some(&label), None).await;
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
`preview` | string | none | A URL for the image on the theme list; the initials stand in without one.
`parent` | string | none | The theme whose templates fill in for the ones this theme lacks.
`require` | list of module ids | `[]` | The modules the theme's templates need.
`form` | form descriptor | none | The theme's own settings fields (see [Descriptor Files](../extend/descriptor-files.md)).

A key outside this table is refused. `ThemeConfig::parse` reads the YAML and `ThemeConfig::to_yaml` writes it.

## The preview image

```yaml
name: Acme
preview: https://acme.test/themes/acme/preview.png
```

Property | Standard
--- | ---
Size | 1200 × 800 pixels. Minimum 600 × 400.
Aspect | 3:2, exactly.
Format | PNG or JPEG. WebP is accepted.
Weight | Under 300 KB.
Subject | The home page as the theme renders it, cropped from the top of the viewport at 1200 pixels wide.
Framing | No browser chrome, device frame, drop shadow, border or rounded corners: the image edges are the page edges.
Overlays | No added text, badges, logos or colour swatches.
Content | Representative content and real images; no placeholder blocks.
Colour scheme | Light, unless the theme is dark only.
Transparency | None: an opaque image.
Shown at | 184 pixels wide on the theme list, so headings and large shapes must still read at that size.

A theme without `preview` shows its initials on the theme list.

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

## Pages

A page's settings name its URL pattern and its layout; the save refuses a pattern that does not parse or a layout the theme and its ancestors lack.

```json
{
  "url": "/blog/:slug",
  "layout": "default",
  "title": "A post",
  "description": "",
  "meta_title": "",
  "meta_description": "",
  "meta_image": "",
  "meta_type": "",
  "meta_robots": "",
  "hidden": false
}
```

Key | Default | Meaning
--- | --- | ---
`url` | required | The URL pattern.
`layout` | none | The layout by name (`default` for `layouts/default.htm`); the page then fills its `page` block, and a block the page defines at its top level fills the layout's block of that name without rendering in place. A page that writes `{% extends %}` itself is left as written.
`title`, `description` | `""` | The page's own words, read as `this.page.title`.
`meta_title`, `meta_description`, `meta_image`, `meta_type`, `meta_robots` | `""` | The head's meta fields.
`hidden` | `false` | Served only to an operator previewing the theme.

Pattern | Matches
--- | ---
`/blog` | `/blog`, in any case
`/blog/:slug` | one segment, bound to `slug`
`/blog/:slug?` | the segment or nothing
`/blog/:page?1` | the segment, or `1` when left out
`/blog/:id|^[0-9]+$` | a segment the expression matches
`/docs/:path*` | one or more segments, joined with `/`
`/docs/:path?*` | none or more segments

An optional parameter before a required one is required. One wildcard at most, with no expression and nothing optional after it. The most specific page wins: more static segments, then fewer wildcards, then fewer parameters.

```jinja
{% block title %}{{ this.page.title }}{% endblock %}
<a href="{{ page('blog/post', slug=post.slug) }}">{{ post.title }}</a>
```

Name | Gives
--- | ---
`page(<name>, param=value, ...)` | The URL of the page named by its path without the extension; a parameter left out takes its default.
`{% block page %}` in a layout | Where the page renders.

```rust
let engine = Engine::build(&db, "acme").await?;
if let Some((name, params)) = engine.resolve("/blog/hello") {
    let settings = engine.page(name);
    let url = engine.url_for(name, &params);
}
for (name, url) in engine.routes().fixed() {
    println!("{name} at {url}");
}
```

`router::Pattern::parse`, `Pattern::matches`, `Pattern::url` and `router::RouteTable` are the engine beneath, usable on their own.

The editor shows a page's settings as fields (URL, a layout picker over the theme and its ancestors, title, description, the meta fields, hidden) and keeps the JSON box for every other kind; the theme page lists each page's URL beside its path.

## The site

`ThemeModule` serves the active theme's pages at every URL the admin and the other public routes leave. A page renders with `this` and `head`:

```jinja
<html>
<head>{{ head }}</head>
<body>
  <h1>{{ this.page.title }}</h1>
  <p>{{ this.params.slug }} on {{ this.url }} in {{ this.theme.name }}</p>
  {% if this.preview %}<p>Previewing.</p>{% endif %}
  {% block page %}{% endblock %}
</body>
</html>
```

Name | Gives
--- | ---
`this.page` | The page's settings and `name`.
`this.params` | The URL's parameters.
`this.theme` | `code` and `name`.
`this.layout` | The layout the page named.
`this.url` | The request path.
`this.preview` | Whether an operator is previewing.
`head` | The title, description, canonical, Open Graph and robots tags from the page's settings; a setting may carry an expression.

Page at | Serves
--- | ---
`/404` | An unmatched URL, with status 404.
`/error` | A page that failed to render, with status 500.

URL | Serves
--- | ---
`/themes/<code>/assets/<path>` | The theme's text asset, rendered, with an ETag and `public, no-cache`.

An operator signed in to the admin with `web.manage_themes` sees their edit theme and its hidden pages, with `private, no-store`; a visitor sees the active theme. With a site registered, the admin's session cookie is scoped to `/` so the site can tell the two apart; without one it stays scoped to the admin mount. The site is registered through `PublicRouteReg::site`, so a module of your own can take its place:

```rust
registry.add_public_route(PublicRouteReg::site(Arc::new(MySite)));
```

```rust
let operator = ctx.operator(&headers).await;     // the session cookie's operator, if any
ctx.record_unserved("GET", "/missing", 404).await; // into the request log
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

## Settings

A theme declares its own settings under `form` in the config; the theme page shows them as a form, and templates read the values beside the theme's keys:

```yaml
name: Acme
form:
  fields:
    site_name:
      label: Site name
      type: text
    tagline:
      label: Tagline
      type: textarea
```

```jinja
<h1>{{ theme.site_name }}</h1>
<p>{{ theme.tagline }}</p>
```

A child theme without a `form` uses its parent's; the values are each theme's own. A field named `code`, `name`, `author`, `homepage` or `description` is refused.

```rust
let chain = store::chain(&db, "acme").await?;
let fields = store::form_fields(&chain);
store::save_settings(&db, "acme", &serde_json::json!({ "site_name": "Acme" })).await?;
let values = store::find_theme(&db, "acme").await?.unwrap().settings;
```

A module's own screen renders and collects such a form through the route context:

```rust
let form = ctx.value_form(&fields, "acme")?;
let markup = form.render(&values, &shell);
let saved = form.collect(&data)?;
```

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
`partial(<path>, name=value, ...)` | A partial with the named variables, the request's `this` and `head`, and the globals.
`content(<path>, name=value, ...)` | A content block: `.md` rendered to HTML, `.htm` and `.html` as they are, anything else escaped as text; `{name}` in the block is the named variable, escaped.
`asset(<path>)` | The URL of the theme's text asset: `/themes/<code>/assets/<path>`.
`url(<path>)` | The absolute URL of a path on the site, from the deployment's origin.
`media(<file>)`, `media(<file>, preset="card")` | The URL of a media value (a URL, or an object with `url` as `theme::media_value` builds), under a preset when one is named.
`<url>\|variant("card")` | A media URL rewritten to the named preset.
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

## Components

A module ships a component; a page or layout attaches it in its settings under `components`, keyed by the alias the template reads it as:

```rust
use lateritecmf::web::theme::component::{
    Component, ComponentContext, ComponentOutput, ComponentReg, PropertyDef, PropertyKind,
};
use lateritecmf::web::theme::ThemeError;
use std::sync::Arc;

struct Posts;

#[lateritecmf::core::strata::async_trait]
impl Component for Posts {
    fn properties(&self) -> Vec<PropertyDef> {
        vec![
            PropertyDef::new("limit", "How many", PropertyKind::Number).default(5),
            PropertyDef::new("slug", "Slug", PropertyKind::Text),
        ]
    }
    async fn run(&self, cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError> {
        let limit = cx.properties["limit"].as_u64().unwrap_or(5);
        let items = load_posts(cx.db, limit).await?;
        Ok(ComponentOutput::data(serde_json::json!({ "items": items }))
            .inject("site_name", "Acme"))
    }
}

fn register(&self, registry: &mut Registry) {
    registry.add(ComponentReg::new("acme.posts", "Posts", Arc::new(Posts)));
}
```

```json
{
  "url": "/blog/:slug",
  "layout": "default",
  "components": {
    "post": { "component": "acme.posts", "limit": 1, "slug": "{{ :slug }}" },
    "latest": { "component": "acme.posts" }
  }
}
```

```jinja
<h1>{{ post.items[0].title }}</h1>
{% for item in latest.items %}<li>{{ item.title }}</li>{% endfor %}
<footer>{{ site_name }}</footer>
```

Name | Gives
--- | ---
`properties()` | The properties an attachment may set, with kind, required and default; the save refuses an unknown component, an undeclared property, a wrong kind or a missing required one.
`run()` | `ComponentOutput::data`, read under the alias, and `.inject(name, value)` for the page scope.
`cx.properties` | The attached values, `{{ :name }}` resolved from the URL and defaults filled.
`cx.db`, `cx.params`, `cx.path`, `cx.theme`, `cx.preview` | The request.

A layout's components run first, then the page's; a page alias replaces a layout's. `build::build_with` takes the registered components for a static build.

A component may ship markup; a theme overrides it per alias:

```rust
fn markup(&self) -> Option<&'static str> {
    Some("<ul>{% for item in component.items %}<li>{{ item.title }}</li>{% endfor %}</ul>")
}
```

```jinja
{{ component('latest') }}
```

Name | Gives
--- | ---
`component(<alias>)` | The theme's `partials/<alias>/default.htm`, else the component's own markup, with the data as `component` and under the alias, beside `this` and `head`.

The page editor's Components box takes the `components` object as JSON and lists the registered components with their properties.

## Markup a module adds

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

Every registration is installed into each engine the site builds, in order. `lat theme build` renders without an application's modules, so a template that needs their markup is built from the application with `build::build_with`.

## Maintenance mode

Settings › Website › Maintenance mode, under `web.manage_site`:

Field | Meaning
--- | ---
Show visitors the maintenance page | On, a visitor gets status 503 with `Retry-After: 600`.
Message | The plain notice's text, when the theme has no page at `/maintenance`.

Page at | Serves
--- | ---
`/maintenance` | Every visitor's request while the mode is on.

An operator with `web.manage_themes` sees the site as usual.

```rust
use lateritecmf::web::theme::maintenance::MaintenanceSettings;

let settings: MaintenanceSettings = lateritecmf::admin::settings::load(&db).await?;
```

## Building a static site

```bash
lat theme build acme
lat theme build acme --out public --base-url https://acme.example
```

Every page whose URL has no parameter is written at its URL (`/about` to `about/index.html`), the `/404` page also as `404.html`, and the text assets under `themes/<code>/assets/`, through the same render path the site serves. A page with parameters or a hidden page is skipped and named.

Flag | Default | Meaning
--- | --- | ---
`--out` | `dist` | The output directory.
`--base-url` | the application's `app.url`, else one derived from `server.listen` | The origin `url()` and the canonical link use.

```rust
use lateritecmf::web::theme::build;

let report = build::build(&db, "acme", Path::new("dist"), "https://acme.example").await?;
println!("{} pages, {} assets, {} skipped", report.pages.len(), report.assets.len(), report.skipped.len());
```

## Export and import

```bash
lat theme list
lat theme export acme
lat theme export acme --out themes/acme.zip
lat theme import acme themes/acme.zip
lat theme import acme themes/acme.zip --replace
```

The theme page has Export as zip and Import a zip; the list has Import a theme from a zip, which creates the theme from the archive's `theme.yaml`.

```text
acme.zip
├── theme.yaml
├── settings.json         # the theme's settings values, when it has any
├── layouts/default.htm
├── pages/home.htm
├── partials/nav.htm
├── content/intro.md
└── assets/css/site.css
```

A template with settings carries them as YAML above a `==` line:

```text
url: /
layout: default
==
{% extends "layouts/default.htm" %}
```

Flag or field | Meaning
--- | ---
`--replace`, Replace templates | Replace the templates and the config the theme already has; each replaced template keeps a revision. Without it, an existing template is skipped.
`code` | The theme to import into; created from `theme.yaml` when it does not exist.

```rust
use lateritecmf::web::theme::archive;

let bytes = archive::export(&db, "acme").await?;
let report = archive::import(&db, "acme-copy", &bytes, false, Some(operator_id)).await?;
println!("{} written, {} skipped", report.imported, report.skipped);
```

`ImportReport` carries `created`, `imported`, `skipped` and `ignored` (files that are not templates).

## The active theme and the edit theme

```rust
theme::set_active_theme(&db, "acme").await?;
let live = theme::active_theme(&db).await?;

theme::set_edit_theme(&db, operator_id, Some("acme-next")).await?;
let editing = theme::edit_theme(&db, operator_id).await?; // the operator's choice, else the active theme
theme::set_edit_theme(&db, operator_id, None).await?;
```

`ThemeSettings` (code `laterite.theme`) holds the active theme; `EditTheme` (code `laterite.theme.edit`) is each operator's preference.
