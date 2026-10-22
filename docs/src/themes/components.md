# Components

A module ships a component; a page or layout attaches it in its settings under `components`, keyed by the alias the template reads it as.

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

A layout's components run first, then the page's; a page alias replaces a layout's.

## Ship markup with a component

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

## Offer a component as a snippet

`ComponentReg::snippet` lists the component in the editors' Insert menu; see [Snippets](snippets.md#back-a-snippet-with-a-component).

## Build a static site with components

`build::build_with` takes the registered components for a static build; see [Static Build](static-build.md#build-from-the-application).
