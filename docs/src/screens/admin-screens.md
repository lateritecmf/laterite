# Admin Screens

A module mounts a screen inside the admin for what a list and a form cannot
express: an import wizard, a report, a diff view.

## Take axum from the framework

```rust
use laterite_admin::axum::{routing::get, Router};
use laterite_core::strata::async_trait;
```

Never add `axum` or `async-trait` to a plugin's `Cargo.toml`.

## Mount an admin screen

```rust
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};

struct Importer;

impl Screen for Importer {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let db = ctx.db().clone();
        Router::new()
            .route("/", get(upload_form).post(run_import))
            .route("/report", get(move || show_report(db.clone())))
    }
}

fn register(&self, registry: &mut Registry) {
    registry.add_screen(
        ScreenReg::new("/import", "acme.import", Arc::new(Importer))
            .in_menu(t!("Import places")),
    );
}
```

`ScreenReg` | Description
--- | ---
`new(path, permission, screen)` | `path` sits under the module's identity: `acme.places` at `/import` serves `/admin/acme/places/import`. `permission` is required and enforced before the handler runs.
`.in_menu(label)` | Adds it to the main menu. Without it the screen still mounts, reached by link.

The screen runs behind the session, CSRF protection and the error pages.

## Render a page in the admin

```rust
use askama::Template;
use laterite_admin::axum::{Extension, Router};
use laterite_admin::html::Markup;
use laterite_admin::{AdminError, Shell};
use laterite_core::t;

#[derive(Template)]
#[template(path = "places.html")]
struct PlacesTemplate<'a> {
    shell: &'a Shell,
    rows: Vec<String>,
}

async fn places(Extension(shell): Extension<Shell>) -> Response {
    let body = PlacesTemplate { shell: &shell, rows: vec![] };
    match Markup::from_template(&body) {
        Ok(body) => shell.page(&t!("Places"), body),
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}
```

```html
<div class="lat-card">
  <form method="post" action="/admin/places">
    <input type="hidden" name="_csrf" value="{{ shell.csrf_token() }}">
    <button class="lat-btn lat-btn--primary" type="submit">{{ shell.t("Save") }}</button>
  </form>
</div>
```

Method | Does
--- | ---
`shell.page(&title, body)` | The body inside the chrome: nav, flash messages, assets, the title as the heading.
`shell.t("Save")`, `shell.tt(&text)` | A string, or a `Text`, in the user's locale.
`shell.csrf_token()` | The request token for a form the template builds. First in an upload form.
`shell.datetime(&utc)` | An RFC 3339 timestamp in the user's timezone.
`shell.base()` | The admin mount, for a link into the panel.

The admin's stylesheet is already on the page: `lat-card`, `lat-table`,
`lat-fields`, `lat-field`, `lat-input`, `lat-select`, `lat-check`, `lat-facts`,
`lat-actions`, `lat-btn`, `lat-alert`, `lat-help`, `lat-status`. A
`SessionHandle` extension carries `push_flash`; an `AdminError` renders the
styled 404, 403 or 500. Headings, dialogs, split panes and chrome builders are
on [Page Layout](page-layout.md).

## Choose where it mounts

```rust
fn admin_base(&self) -> Option<&'static str> { Some("/places") }
```

```toml
[backend.paths]
"acme.places" = "/geography"
"acme.places/import" = "/geography/bulk-upload"
```

The deployment has the last word. Two modules on one path, or one on a
framework screen, abort the boot naming both.

## Link to yourself

```rust
ctx.url("/report")      // "/admin/places/import/report", wherever it mounted
ctx.base_url()          // the site origin, from app.url or the bind address
```

`base_url` is for a URL a route emits: a sitemap `<loc>`, a canonical, a feed
entry. Never build one from the `Host` header.

## Add middleware

```rust
fn mount(&self, _ctx: &RouteCtx) -> Router {
    Router::new()
        .route("/", get(robots_txt))
        .layer(CompressionLayer::new())
}
```

## Read other modules' contributions

```rust
pub struct FeedReg {
    pub name: String,
}

impl PublicRoute for Feeds {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let feeds: Vec<&FeedReg> = ctx.contributions::<FeedReg>();
        // ...
    }
}
```

Any `Send + Sync` type another module registers. Contributions the framework
consumes itself (resources, screens, settings, permissions, persisters,
listeners, column types, public routes) are not visible here.

## Write through a resource's form

```rust
use std::collections::HashMap;
use laterite_admin::persist::SaveError;
use laterite_core::Actor;

impl Screen for Importer {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let products = ctx.form("/products").expect("the products resource");
        Router::new().route("/run", post(move || {
            let products = products.clone();
            async move {
                let actor = Actor::system("acme.import");
                let mut row = HashMap::new();
                row.insert("sku".to_string(), "A-100".to_string());
                row.insert("name".to_string(), "Anvil".to_string());
                match products.create(row, &actor).await {
                    Ok(id) => format!("created {id}"),
                    Err(SaveError::Invalid(bag)) => format!("refused: {bag:?}"),
                    Err(SaveError::Failed(e)) => format!("failed: {e}"),
                }
            }
        }))
    }
}
```

`ctx.form(path)` takes the resource's path as its descriptor declares it, relative
to the admin root, and is `None` when nothing with a form mounts there.

Method | Does
--- | ---
`create(data, &actor)` | Validates `data` as a submission, saves a new record through the form's field types, listeners and persister, and returns its id.
`update(id, data, &actor)` | The same as an edit of the record `id`. Every field is written; one left out of `data` is written blank.
`entity()`, `id_field()` | The table and its primary key column.
`fields()` | The declared fields, in order; `data` is keyed by their names.

`SaveError::Invalid` carries the per-field messages a refused submit shows;
`SaveError::Failed` is the write failing. The audit log records the actor.
