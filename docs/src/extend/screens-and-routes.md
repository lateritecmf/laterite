# Screens and Routes

A module mounts its own routes: a screen inside the admin, or a public endpoint
outside it. A screen is for what a list and a form cannot express: an import
wizard, a report, a diff view.

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
`new(path, permission, screen)` | `path` sits under the module's identity: `rainmill.location` at `/import` serves `/admin/rainmill/location/import`. `permission` is required and enforced before the handler runs.
`.in_menu(label)` | Adds it to the main menu. Without it the screen still mounts, reached by link.

The screen inherits the session, the operator, CSRF protection and the error
pages.

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
`shell.t("Save")`, `shell.tt(&text)` | A string, or a `Text`, in the operator's locale.
`shell.csrf_token()` | The request token for a form the template builds. First in an upload form.
`shell.datetime(&utc)` | A stored RFC 3339 timestamp in the operator's timezone.
`shell.base()` | The admin mount, for a link into the panel.

The admin's stylesheet is already on the page: `lat-card`, `lat-table`,
`lat-fields`, `lat-field`, `lat-input`, `lat-select`, `lat-check`, `lat-facts`,
`lat-actions`, `lat-btn`, `lat-alert`, `lat-help`, `lat-status`. A
`SessionHandle` extension carries `push_flash`; an `AdminError` renders the
styled 404, 403 or 500.

## Choose where it mounts

```rust
fn admin_base(&self) -> Option<&'static str> { Some("/places") }
```

```toml
[backend.paths]
"rainmill.location" = "/geography"
"rainmill.location/import" = "/geography/bulk-upload"
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

## Mount a public route

```rust
use laterite_admin::routes::{PublicRoute, PublicRouteReg, RouteCtx};

struct Robots;

impl PublicRoute for Robots {
    fn mount(&self, _ctx: &RouteCtx) -> Router {
        Router::new().route("/", get(robots_txt))
    }
}

registry.add_public_route(PublicRouteReg::new("/robots.txt", Arc::new(Robots)));
```

No session, no permission, no admin chrome. The path is literal, never
namespaced; a route inside the admin mount is refused. A POST decides its own
CSRF stance.

## Add middleware

```rust
fn mount(&self, _ctx: &RouteCtx) -> Router {
    Router::new()
        .route("/", get(robots_txt))
        .layer(CompressionLayer::new())
}
```

The framework's session, permission, CSRF and panic layers wrap yours from the
outside.

## Test a route

```rust
let ctx = RouteCtx::builder(db)
    .admin_path("/backoffice")
    .base_url("https://acme.example")
    .build();

let response = Robots.mount(&ctx)
    .oneshot(Request::get("/").body(Body::empty())?)
    .await?;
```

Every builder field has a default; name only what you assert on.

## Put a checklist on a screen

```rust
use laterite_admin::checklist::{Checklist, Choice, Expand, Offer};
use laterite_core::Translator;

let list = Checklist::new("Channels")
    .choice(Choice::new("channel", "email", "Email").checked(true))
    .group("Chat", vec![
        Choice::new("channel", "slack", "Slack").note("One workspace").into(),
        Choice::new("channel", "teams", "Teams").locked(true).into(),
    ])
    .select_all(Offer::Auto)
    .search(Offer::Never)
    .expand(Expand::All);

// `Markup`, for a template's `|safe` slot
let html = list.render(&Translator::new("en"));
```

Builder | Does
--- | ---
`Choice::new(name, value, label)` | One box, submitted as `name=value`.
`.note(..)`, `.code(..)` | A second line under the label; an identifier beside it.
`.checked(..)`, `.locked(..)` | Ticked; shown and not changeable.
`.group(label, entries)` | A heading over entries of its own, nested to any depth.
`.select_all(..)`, `.search(..)`, `.expand(..)` | The defaults listed under [Behaviours](../reference/behaviours.md).
`.read_only(true)` | Every box locked.
`.view(&translator)` | The resolved list as a serialisable value, for a field type's view-model.

## Contribute a column type

```rust
use laterite_admin::html::Markup;
use laterite_admin::list::{CellCx, CellVm, ColumnType, ColumnTypeReg};

struct Rating;

impl ColumnType for Rating {
    fn view_key(&self) -> &'static str { "acme.rating" }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm { /* ... */ }
    fn render_default(&self, vm: &CellVm) -> Markup { /* ... */ }
}

registry.add(ColumnTypeReg::new(Arc::new(Rating)));
```

Use a dotted `vendor.name` key. A taken key aborts the boot; so does a column
naming a key nobody registered.

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
to the admin root, and is `None` when nothing with a form mounts there. The
handle is `Clone` and owns what it needs.

Method | Does
--- | ---
`create(data, &actor)` | Validates `data` as a submission, saves a new record through the form's field types, listeners and persister, and returns its id.
`update(id, data, &actor)` | The same as an edit of the record `id`. Every field is written; one left out of `data` is stored blank.
`entity()`, `id_field()` | The table and its primary key column.
`fields()` | The declared fields, in order; `data` is keyed by their names.

`SaveError::Invalid` carries the per-field messages a refused submit shows;
`SaveError::Failed` is the write failing. The audit log records the actor.
