# Screens and Routes

A module contributes its own routes when descriptors cannot express what it needs:
a screen inside the admin, or a public endpoint outside it.

Reach for a resource first. Lists and forms are data, and the generic handlers
render them with validation, pagination and a write path already wired. A screen
is for what a list and a form cannot be: an import wizard, a calendar, a report
builder, a diff view.

## An admin screen

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

The framework mounts it inside the admin, so it inherits the session and the
authenticated operator, the permission you declared, CSRF protection, and the
styled error pages. The permission is enforced by the framework before your
handler runs, so a screen cannot forget its own gate. It is required: a screen
without one aborts the boot.

## Where a screen mounts

By default under the module's own identity, so `rainmill.location` contributing
`/import` serves `/admin/rainmill/location/import`. Two modules therefore cannot
collide by accident.

A module can take a shorter path instead:

```rust
fn admin_base(&self) -> Option<&'static str> { Some("/places") }
```

and a deployment has the last word, in config:

```toml
[backend.paths]
"rainmill.location" = "/geography"
"rainmill.location/import" = "/geography/bulk-upload"
```

Taking a short path is a claim. If two modules end up on one path, or one lands on
a framework screen, the application refuses to start and names both, rather than
one silently shadowing the other.

## Linking to yourself

A screen cannot know its own URL: a deployment can move it. Build every self-link
from the context:

```rust
ctx.url("/report")   // "/admin/places/import/report", wherever it ended up
```

The same applies inside templates and JavaScript: pass the base in, and read
endpoints from a data attribute rather than hardcoding a path.

## A public route

For the endpoints that must live at a literal path, outside the admin:

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

Public means public: no session, no permission, no admin chrome, though the styled
error pages still apply. The path is literal and never namespaced, because
`/robots.txt` has to be exactly that, so collisions are likelier and the same boot
check applies. A route that reaches inside the admin mount is refused.

A public route that accepts a POST decides its own stance on CSRF. The admin's
blanket protection assumes a session, which a webhook does not have.

## Finding the admin

A module that needs to know where the panel ended up, to build a link into it or
to exclude it from a sitemap, asks rather than assuming, since an operator may
have moved it with `backend.path` or `backend.paths`.

One note on robots files specifically: listing the admin path tells anyone reading
it where your panel is, and authentication already keeps crawlers out. Silence is
the stronger choice.
