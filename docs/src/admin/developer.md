# Developer Section

With `app.debug` on, a superuser sees a Developer section in the admin's navigation: what the application registered, from the running process.

```toml
[app]
debug = true
```

Screen | Shows
--- | ---
Routes | Every URL the application answers, with method, kind, owner and permission: the list `lat routes` prints.
Field types | Every registered field type, rendered once with its defaults.
Mail previews | Every mail template and layout, each with its preview.
Registries | What every module contributed, by registry: resources, screens, settings, permissions, field and column types, widgets, public routes.

The section is absent when `app.debug` is off, and a 403 to an operator who is not a superuser. See [Application and Server](../configuration/application.md) for `app.debug`.
