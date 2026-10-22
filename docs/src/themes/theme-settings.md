# Theme Settings

A theme declares its own settings under `form` in the config; the theme page shows them as a form, and templates read the values beside the theme's keys.

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

Key | Meaning
--- | ---
`form` | A form descriptor (see [Form Descriptor](../forms/form-descriptor.md)); the fields are listed under [Form Fields](../elements/form-fields.md).
`theme.<field>` | The saved value, read in any template beside `theme.code`, `theme.name`, `theme.author`, `theme.homepage` and `theme.description` (see [Properties](../markup/properties.md)).

A child theme without a `form` uses its parent's; the values are each theme's own. A field named `code`, `name`, `author`, `homepage` or `description` is refused.

## Read and save the values from Rust

```rust
let chain = store::chain(&db, "acme").await?;
let fields = store::form_fields(&chain);
store::save_settings(&db, "acme", &serde_json::json!({ "site_name": "Acme" })).await?;
let values = store::find_theme(&db, "acme").await?.unwrap().settings;
```

## Render the form on a module screen

A module's own screen renders and collects such a form through the route context:

```rust
let form = ctx.value_form(&fields, "acme")?;
let markup = form.render(&values, &shell);
let saved = form.collect(&data)?;
```
