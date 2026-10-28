# Filters

A filter follows a value after `|` and transforms it before it prints.

```jinja
{{ post.body|content }}
{{ post.summary|markdown }}
<img src="{{ post.image|variant('card') }}">
{{ price|money }}
```

Name | Gives
--- | ---
`<html>\|content` | Snippets in stored HTML expanded (see [Snippets](../themes/snippets.md)); `content(<path>)` does the same for a block.
`<text>\|markdown` | Markdown rendered to HTML: CommonMark with tables, strikethrough, footnotes, task lists and `{#id}` heading attributes; a heading without one gets an id from its text (`## Release 0.9` is `release-0-9`); raw HTML passes through.
`<url>\|variant("card")` | A media URL rewritten to the named preset (see [Image Variants](../media/variants.md)).

## Add a filter from a module

```rust
impl Markup for Money {
    fn install(&self, env: &mut Environment<'static>) {
        env.add_filter("money", |minor: i64| format!("{}.{:02}", minor / 100, minor % 100));
    }
}
```

The registration is described under [Templating](templating.md#add-markup-from-a-module).
