# Testing

How a test drives a contributed route without a running server.

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

Builder | Default
--- | ---
`base_path(..)` | `/`, where the screen mounted.
`admin_path(..)` | `/admin`, where the panel mounted.
`base_url(..)` | `http://localhost`, the site's origin.
`contributions(registry)` | Empty: what `ctx.contributions` reads.
`events(events)` | A bus nothing listens on.
`auth(service)` | A default service over the same database.

Every builder field has a default; name only what you assert on.
