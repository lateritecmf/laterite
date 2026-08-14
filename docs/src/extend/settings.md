# Settings Models

A settings model holds a group of configuration values that an operator edits
from the admin panel: a site title, a page size, feature toggles. In Laterite a
settings model is a plain Rust struct. It is stored as a single JSON value keyed
by a stable code, so adding or removing a field never needs a database
migration, and access to it is checked by the compiler.

This is provided by the `laterite-settings` crate.

## Define a settings model

Derive `Serialize`, `Deserialize`, and `Default`, then implement
`SettingsModel` with a stable `CODE`. Give every field `#[serde(default)]` so a
stored value that predates a new field still deserializes:

```rust
use laterite_settings::SettingsModel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SiteSettings {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub per_page: u32,
}

impl SettingsModel for SiteSettings {
    const CODE: &'static str = "acme.site";
}
```

The `CODE` is the storage key. Choose it once and never change it, the way you
would treat a table name. Namespacing it to your application (`acme.site`)
keeps it from colliding with settings declared by other modules.

## Register the migration

The settings table is created by the crate's own migration. Add its set to the
application's migration runner once, alongside the other modules:

```rust
laterite_core::migrate::run(
    &pool,
    &[laterite_settings::migrations()],
)
.await?;
```

## Read and write, typed

Load a model with `load`. When nothing has been saved yet, it returns the
model's `Default` rather than an error, so callers never handle an "unset"
case:

```rust
let settings: SiteSettings = laterite_settings::load(&pool).await?;
println!("{}", settings.title);
```

Save with `save`, which upserts the whole struct as one JSON value:

```rust
let settings = SiteSettings {
    title: "Acme".into(),
    per_page: 25,
};
laterite_settings::save(&pool, &settings).await?;
```

## Read and write, untyped

The admin settings screen renders and stores any registered model without
knowing its concrete type. For that path, `get` and `set` work over a raw
`serde_json::Value` keyed by code:

```rust
let value = laterite_settings::get(&pool, SiteSettings::CODE).await?;
laterite_settings::set(&pool, "acme.site", &value.unwrap_or_default()).await?;
```

## Editing settings in the admin

To let an operator edit a model from the admin panel, register a
`SettingsItem` describing where it appears and which fields to show, and pass it
to the admin router. One generic screen lists every registered item grouped by
category, and one generic form edits each, reading and writing the JSON value
through `get`/`set`. No per-model controller is needed.

```rust
use laterite_admin::settings::{SettingsField, SettingsItem};

let site = SettingsItem {
    code: SiteSettings::CODE.to_string(),
    label: "Site".to_string(),
    description: "Public site title and page size.".to_string(),
    category: "General".to_string(),
    order: 10,
    permission: None,
    link: None,
    fields: vec![
        SettingsField::text("title", "Site title"),
        SettingsField::text("per_page", "Items per page"),
    ],
};

let app = laterite_admin::router(
    auth,
    pool,
    Vec::new(),
    vec![site],
    laterite_admin::AdminConfig::default(),
);
```

Fields carry a widget: `SettingsField::text`, `::textarea`, or `::switch` (a
checkbox stored as a JSON boolean). Items sort by `category`, then `order`. The
model's migration must be registered (above) so the `settings` table exists.

## The settings menu vs the main menu

The admin has two menus. The **main menu** (top nav) holds Dashboard, the
application's own sections, and Settings. The **settings menu** is the grouped
index at `/admin/settings`. Administrative screens (backend users, roles, and the
like) belong in the settings menu, not as top-level tabs.

A `SettingsItem` normally edits a settings model at `/admin/settings/{code}`. Set
its `link` to place an existing screen (a resource list) in the settings menu
instead of a form:

```rust
SettingsItem {
    code: "acme.pages".to_string(),
    label: "Pages".to_string(),
    description: "Manage site pages.".to_string(),
    category: "Content".to_string(),
    order: 10,
    permission: None,
    link: Some("/admin/pages".to_string()),
    fields: Vec::new(),
};
```

The framework registers its own Users and Roles this way, under a Users category.

## Evolving a model

Because a model is one JSON blob and every field is `#[serde(default)]`:

- **Adding a field** needs no migration. Existing rows lack the key, so it
  deserializes to the field's default until the operator saves again.
- **Removing a field** needs no migration. The stale key in stored JSON is
  ignored on load.
- **Renaming a field** is a data change, not a schema change. Treat it like any
  rename: read the old key, write the new one. Never reuse a `CODE` for an
  incompatible model.
