# Public Routes

A module mounts an endpoint outside the admin: a feed, a `robots.txt`, a
webhook. No session, no permission, no admin chrome.

## Mount a public route

```rust
use laterite_admin::axum::{routing::get, Router};
use laterite_admin::routes::{PublicRoute, PublicRouteReg, RouteCtx};

struct Robots;

impl PublicRoute for Robots {
    fn mount(&self, _ctx: &RouteCtx) -> Router {
        Router::new().route("/", get(robots_txt))
    }
}

registry.add_public_route(PublicRouteReg::new("/robots.txt", Arc::new(Robots)));
```

`PublicRouteReg` | Description
--- | ---
`new(path, route)` | `path` is literal, never namespaced. A route inside the admin mount is refused.

A POST decides its own CSRF stance. `ctx.url`, `ctx.base_url`, middleware and
other modules' contributions work as on an
[admin screen](admin-screens.md#link-to-yourself).
