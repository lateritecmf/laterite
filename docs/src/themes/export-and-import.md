# Export and Import

A theme travels as a zip of its config, settings values and templates, written by `lat theme export` and read by `lat theme import`.

```bash
lat theme list
lat theme export acme
lat theme export acme --out themes/acme.zip
lat theme import acme themes/acme.zip
lat theme import acme themes/acme.zip --replace
```

```text
acme.zip
├── theme.yaml
├── settings.json         # the theme's settings values, when it has any
├── layouts/default.htm
├── pages/home.htm
├── partials/nav.htm
├── content/intro.md
└── assets/css/site.css
```

A template with settings carries them as YAML above a `==` line:

```text
url: /
layout: default
==
{% extends "layouts/default.htm" %}
```

Flag or field | Meaning
--- | ---
`--replace`, Replace templates | Replace the templates and the config the theme already has; each replaced template keeps a revision. Without it, an existing template is skipped.
`code` | The theme to import into; created from `theme.yaml` when it does not exist.

## Export and import from Rust

```rust
use lateritecmf::web::theme::archive;

let bytes = archive::export(&db, "acme").await?;
let report = archive::import(&db, "acme-copy", &bytes, false, Some(operator_id)).await?;
println!("{} written, {} skipped", report.imported, report.skipped);
```

`ImportReport` carries `created`, `imported`, `skipped` and `ignored` (files that are not templates).
