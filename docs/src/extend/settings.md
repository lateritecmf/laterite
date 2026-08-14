# Settings Models

A settings model holds a group of configuration values that an operator edits
from the admin panel: a site title, a tagline, feature toggles. In Laterite a
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
    pub tagline: String,
    #[serde(default)]
    pub maintenance_mode: bool,
}

impl SettingsModel for SiteSettings {
    const CODE: &'static str = "acme.site";
}
```

The `CODE` is the storage key. Choose it once and never change it, the way you
would treat a table name. Namespacing it to your application (`acme.site`)
keeps it from colliding with settings declared by other modules.

## The settings table

The settings store lives in a single `settings` table.
`laterite_admin::builtin_migrations()` already includes its migration, so once
you have [run migrations](../getting-started/installation.md#run-migrations) the
table is there.

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
    tagline: "We build things".into(),
    maintenance_mode: false,
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
    description: "Public site title, tagline, and a maintenance switch.".to_string(),
    category: "General".to_string(),
    order: 10,
    icon: Some("sliders-horizontal".to_string()),
    permission: None,
    link: None,
    fields: vec![
        SettingsField::text("title", "Site title"),
        SettingsField::text("tagline", "Tagline"),
        SettingsField::switch("maintenance_mode", "Maintenance mode"),
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
`settings` table comes from `builtin_migrations()` (above).

Set an item's `permission` to a dotted string to hide it from operators who lack
it; a `None` permission is always visible, and superusers see everything.
Registered items render in a categorised context sidebar on the settings
screens, with the open item highlighted. Give each item an `icon` (a Lucide name
such as `users` or `shield`) for the sidebar; an unknown or `None` name falls
back to a generic glyph.

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
    icon: Some("folder".to_string()),
    permission: None,
    link: Some("/admin/pages".to_string()),
    fields: Vec::new(),
};
```

The framework registers its own Users and Roles this way, under a Users category.

A linked screen keeps the settings context sidebar. The framework derives the
context from the `link`: any request whose path is the link or falls under it
(its list, forms and sub-pages) renders the settings sidebar with that item
active. Registering the item is the only step, and the sidebar tracks the same
link it navigates to.

## Evolving a model

Because a model is one JSON blob and every field is `#[serde(default)]`:

- **Adding a field** needs no migration. Existing rows lack the key, so it
  deserializes to the field's default until the operator saves again.
- **Removing a field** needs no migration. The stale key in stored JSON is
  ignored on load.
- **Renaming a field** is a data change, not a schema change. Treat it like any
  rename: read the old key, write the new one. Never reuse a `CODE` for an
  incompatible model.
