# Functions

A function is called in `{{ }}` and gives a URL, a rendered partial, a content block or a component's markup.

```jinja
<a href="{{ page('blog/post', slug=post.slug) }}">{{ post.title }}</a>
{{ partial("card.htm", title="Acme", body=page.body) }}
{{ content("intro.md") }}
<link rel="stylesheet" href="{{ asset('css/site.css') }}">
<link rel="canonical" href="{{ url(this.url) }}">
<img src="{{ media(post.image, preset='card') }}">
{{ component('latest') }}
```

Name | Gives
--- | ---
`page(<name>, param=value, ...)` | The URL of the page named by its path without the extension; a parameter left out takes its default (see [Pages](../themes/pages.md)).
`partial(<path>, name=value, ...)` | A partial with the named variables, the request's `this` and `head`, and the globals.
`content(<path>, name=value, ...)` | A content block: `.md` rendered to HTML, `.htm` and `.html` as they are, anything else escaped as text; `{name}` in the block is the named variable, escaped (see [Content](../themes/content.md)).
`asset(<path>)` | The URL of the theme's text asset: `/themes/<code>/assets/<path>`.
`url(<path>)` | The absolute URL of a path on the site, from the deployment's origin.
`media(<file>)`, `media(<file>, preset="card")` | The URL of a media value (a URL, or an object with `url` as `theme::media_value` builds), under a preset when one is named.
`component(<alias>)` | The theme's `partials/<alias>/default.htm`, else the component's own markup, with the data as `component` and under the alias, beside `this` and `head` (see [Components](../themes/components.md)).

## Add a function from a module

```rust
impl Markup for Money {
    fn install(&self, env: &mut Environment<'static>) {
        env.add_function("year", || 2026);
    }
}
```

```jinja
{{ year() }}
```

The registration is described under [Templating](templating.md#add-markup-from-a-module). A [static build](../themes/static-build.md) sets the origin `url()` uses with `--base-url`.
