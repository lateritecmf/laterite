# Maintenance Mode

Maintenance mode answers every visitor with the theme's `/maintenance` page, or a plain notice, until it is switched off.

```rust
use lateritecmf::web::theme::maintenance::MaintenanceSettings;

let settings: MaintenanceSettings = lateritecmf::admin::settings::load(&db).await?;
```

Settings › Website › Maintenance mode, under `web.manage_site`:

Field | Meaning
--- | ---
Show visitors the maintenance page | On, a visitor gets status 503 with `Retry-After: 600`.
Message | The plain notice's text, when the theme has no page at `/maintenance`.

Page at | Serves
--- | ---
`/maintenance` | Every visitor's request while the mode is on.

## Keep working while it is on

A user with `web.manage_themes` sees the site as usual.
