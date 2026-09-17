# Settings Models

A settings model is a struct an operator edits from the admin panel.

## Define one

```rust
use laterite_admin::settings::SettingsModel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SiteSettings {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tagline: String,
    #[serde(default)]
    pub maintenance_mode: bool,
}

impl SettingsModel for SiteSettings {
    const CODE: &'static str = "acme.site";
}
```

Derive `Serialize`, `Deserialize` and `Default`, and give every field
`#[serde(default)]`. `CODE` is the storage key: namespace it to your
application and never change it.

## Read and write

```rust
let settings: SiteSettings = laterite_admin::settings::load(&db).await?;

let settings = SiteSettings { title: "Acme".into(), ..Default::default() };
laterite_admin::settings::save(&db, &settings).await?;
```

`load` returns `Default` when nothing is saved. `save` upserts the whole
struct. By code, untyped:

```rust
let value = laterite_admin::settings::get(&db, SiteSettings::CODE).await?;
laterite_admin::settings::set(&db, "acme.site", &value.unwrap_or_default()).await?;
```

## Edit it in the admin

Register a `SettingsItem` from your module's `register`:

```rust
use laterite_admin::form::FormField;
use laterite_admin::settings::SettingsItem;

registry.add_settings(
    SettingsItem::new("acme.site", "Site", vec![
        FormField::text("title", "Site title"),
        FormField::text("tagline", "Tagline"),
        FormField::switch("maintenance_mode", "Maintenance mode"),
    ])
    .description("Public site title, tagline, and a maintenance switch.")
    .category("General")
    .order(10)
    .icon("sliders-horizontal")
    .permission("acme.manage_site"),
);
```

One screen lists every item by category; one form edits each.

Builder | Description
--- | ---
`new(code, label, fields)` | The model's `CODE`, the menu label, and its fields.
`description(text)` | Under the label in the settings index. At most 72 characters.
`hint(text)` | Above the form.
`category(text)` | Groups items in the index.
`order(n)` | Sort within the category.
`icon(name)` | An [icon](../reference/icons.md). An unknown name stops the boot.
`permission(code)` | Hides the item from operators without the grant.
`link(path)` | Places an existing screen in the settings menu, with no form.

Fields are ordinary [`FormField`](https://docs.rs/laterite-admin)s: `text`,
`textarea`, `switch`, `select`, `date`, `repeater`, and any registered type
through `FormField::of(name, label, "vendor.type")`. A type that refuses its
input refuses the save.

## Link a screen into the settings menu

```rust
registry.add_settings(
    SettingsItem::new("acme.pages", "Pages", Vec::new())
        .description("Manage site pages.")
        .category("Content")
        .icon("folder")
        .link("/pages"),
);
```

The path is under the admin mount. Any request at or under it renders the
settings sidebar with the item active. The built-in Users and Roles register
this way.

## Change a model later

Change | Migration
--- | ---
Add a field | None. A missing key deserializes to the field's default.
Remove a field | None. A stale key is ignored.
Rename a field | Read the old key, write the new one. Never reuse a `CODE` for an incompatible model.
