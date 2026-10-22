# Pages

A page is a template served at its URL; its settings name the URL pattern, the layout, the head's meta fields and whether it is hidden.

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
`components` | none | The components attached to the page, by alias (see [Components](components.md)).

The save refuses a pattern that does not parse or a layout the theme and its ancestors lack.

## Write a URL pattern

Pattern | Matches
--- | ---
`/blog` | `/blog`, in any case
`/blog/:slug` | one segment, bound to `slug`
`/blog/:slug?` | the segment or nothing
`/blog/:page?1` | the segment, or `1` when left out
`/blog/:id|^[0-9]+$` | a segment the expression matches
`/docs/:path*` | one or more segments, joined with `/`
`/docs/:path?*` | none or more segments

An optional parameter before a required one is required. One wildcard at most, with no expression and nothing optional after it. The most specific page wins: more static segments, then fewer wildcards, then fewer parameters. The parameters are read as `this.params` (see [Properties](../markup/properties.md)).

## Link to a page

```jinja
{% block title %}{{ this.page.title }}{% endblock %}
<a href="{{ page('blog/post', slug=post.slug) }}">{{ post.title }}</a>
```

Name | Gives
--- | ---
`page(<name>, param=value, ...)` | The URL of the page named by its path without the extension; a parameter left out takes its default.
`{% block page %}` in a layout | Where the page renders (see [Layouts and Partials](layouts-and-partials.md)).

The other functions are listed under [Functions](../markup/functions.md).

## Resolve a URL from Rust

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

## Serve the error pages

Page at | Serves
--- | ---
`/404` | An unmatched URL, with status 404.
`/error` | A page that failed to render, with status 500.

## Hide a page

```json
{ "url": "/drafts", "layout": "default", "hidden": true }
```

A hidden page is served only to a user previewing their [edit theme](themes.md#set-the-active-and-edit-theme), and the [static build](static-build.md) skips it.
