# Errors

Admin handlers return `Result<_, laterite_admin::AdminError>`.

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
the page shows it too.

## Validation

A refused submission is not an `AdminError`: it answers `422` with the form
re-rendered. See [Validation](validation.md).
