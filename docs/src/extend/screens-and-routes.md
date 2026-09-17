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

Use a dotted `vendor.name` key. A taken key aborts the boot.

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
