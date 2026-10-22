# Content

A content block is a file of HTML, Markdown or text a template inserts with `content()`; the `content` filter expands snippets in stored HTML.

```jinja
{{ content("intro.md") }}
{{ content("welcome.htm", name=this.params.name) }}
{{ post.body|content }}
```

```html
{# content/welcome.htm #}
<p>Welcome, {name}.</p>
```

Name | Gives
--- | ---
`content(<path>, name=value, ...)` | A content block: `.md` rendered to HTML, `.htm` and `.html` as they are, anything else escaped as text; `{name}` in the block is the named variable, escaped.
`<html>\|content` | Snippets in stored HTML expanded (see [Snippets](snippets.md)); `content(<path>)` does the same for a block.

Extension | Rendered as
--- | ---
`.md` | HTML, as the [`markdown` filter](../markup/filters.md) renders it.
`.htm`, `.html` | As written.
Any other | Text, escaped.

## Save a content block from Rust

```rust
store::save_template(&db, site.id, Kind::Content, "intro.md", None, "# Welcome\n", None).await?;
```

The path rules and the other calls are under [Themes](themes.md#save-a-template).
