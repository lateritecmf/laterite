# Child Themes

A child theme names a parent in its config and uses the parent's templates for the ones it lacks.

```yaml
name: Acme child
parent: acme
```

```rust
let child = store::create_child_theme(&db, "acme", "acme-child", "Acme child").await?;
```

Key | Type | Default | Meaning
--- | --- | --- | ---
`parent` | string | none | The theme whose templates fill in for the ones this theme lacks.

Lookup | Result
--- | ---
A template by kind and path | The theme's own, else the nearest ancestor's.
A page's `layout` | Any layout of the theme or its ancestors.
The settings `form` | The theme's own, else its parent's; the values are each theme's own (see [Theme Settings](theme-settings.md)).

## Resolve a template through the chain

```rust
// Through the parent chain: the theme's own row, else the nearest ancestor's.
if let Some((from, template)) = store::resolve_template(&db, "child", Kind::Layout, "default.htm").await? {
    println!("{} from {}", template.path, from.code);
}

let chain = store::chain(&db, "child").await?;
```

## Change or remove a parent

```rust
store::update_theme_config(&db, "acme-child", "name: Acme child\nparent: base\n").await?;
```

Call | Refused when
--- | ---
`create_theme` | The parent is missing or leads back to the theme.
`update_theme_config` | The theme is locked, or the parent is missing or leads back.
`delete_theme` | The theme is another theme's parent.

The other calls are under [Themes](themes.md#manage-themes).
