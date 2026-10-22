# Errors and Logging

Admin handlers return `Result<_, laterite_admin::AdminError>`; the log follows
`RUST_LOG`.

Variant | Status | Page
--- | --- | ---
`NotFound` | 404 | Not found
`Forbidden` | 403 | Forbidden
`Internal(_)` | 500 | Something went wrong

## Map a domain error

```rust
async fn edit(state: &AdminState, id: &str) -> Result<Response, AdminError> {
    let row = fetch(state, id)
        .await?                        // CoreError::NotFound -> 404, DB error -> 500
        .ok_or(AdminError::NotFound)?; // a missing row -> 404
    Ok(render(view(row)))
}
```

From | To
--- | ---
`CoreError::NotFound` | 404
`CoreError::Forbidden`, `CoreError::Unauthorized` | 403
`AuthError` | Through `CoreError`, the same mapping
A database error, a render failure, a panic | 500

An unmatched admin URL renders the 404 page.

## Show the cause in development

```toml
[app]
debug = true   # off by default
```

A 500 logs its cause through `tracing::error!` in every mode. With `debug` on,
the page shows it too. See [Application and Server](application.md).

## Configure logging

```bash
RUST_LOG=debug lat serve
RUST_LOG=info,laterite_admin=debug ./acme
```

Variable | Description
--- | ---
`RUST_LOG` | A `tracing` filter directive. Unset, the filter is `info,sqlx=warn`.

An application that installs its own `tracing` subscriber before `serve()`
keeps it.

## Refuse a submission

A refused submission is not an `AdminError`: it answers `422` with the form
re-rendered. See [Validation](../forms/validation.md).
