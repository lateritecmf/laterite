# Editor Commands and Snippets

A module adds entries to the Insert menu of the rich and markdown editors: a
command that inserts fixed content, or a snippet with properties the user fills.

## Add a command

```rust
use laterite_admin::editor::{EditorCommand, Insert, Surface};
use laterite_admin::AdminRegistry;

fn register(&self, registry: &mut laterite_core::Registry) {
    registry.add_editor_command(EditorCommand::new(
        Surface::Rich,
        "acme.callout",
        t!("Callout"),
        Insert::Html(r#"<blockquote><p>Note</p></blockquote>"#.into()),
    ));
    registry.add_editor_command(EditorCommand::new(
        Surface::Markdown,
        "acme.callout",
        t!("Callout"),
        Insert::Text("> **Note**\n> ".into()),
    ));
}
```

Field | Values
--- | ---
`Surface` | `Rich` (the `richeditor` field, inserts HTML), `Markdown` (the `markdown` field, inserts text)
`key` | stable within the surface; a later registration with the same key replaces the earlier
`label` | a `Text`, localized for the user
`Insert` | `Html(..)` for `Rich`, `Text(..)` for `Markdown`

## Offer snippets

```rust
use laterite_admin::editor::{Snippet, SnippetProperty, SnippetPropertyKind, SnippetSource};

struct Embeds;

#[laterite_core::strata::async_trait]
impl SnippetSource for Embeds {
    async fn list(&self, _db: &laterite_core::Db) -> Vec<Snippet> {
        vec![Snippet {
            code: "acme.map".into(),
            name: "Map".into(),
            description: Some("A map centred on an address".into()),
            properties: vec![SnippetProperty {
                code: "address".into(),
                title: "Address".into(),
                kind: SnippetPropertyKind::String,
                default: None,
                options: Vec::new(),
            }],
        }]
    }
}

registry.add_snippet_source(std::sync::Arc::new(Embeds));
```

Field | Values
--- | ---
`SnippetPropertyKind` | `String`, `Dropdown` (with `options` as `(value, label)` pairs), `Checkbox`
`list(db)` | asked each time an editor opens its Insert menu; the list may change while the application runs

An inserted snippet is
`<figure data-snippet="<code>" data-properties="{...}"></figure>`. A theme
declares snippets on its partials and renders them with the `content` filter; a
component is offered with `ComponentReg::snippet` and runs after the page
renders; see [Snippets](../themes/snippets.md). A module that lists snippets
from a source renders them on its own pages the same way.

## Read the menu

```
GET /admin/editor/menu?surface=richeditor
{ "commands": [{ "key": "acme.callout", "label": "Callout", "html": "..." }],
  "snippets": [{ "code": "acme.map", "name": "Map", "properties": [...] }] }
```
