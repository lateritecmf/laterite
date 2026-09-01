# Errors

Admin handlers return `Result<_, laterite_admin::AdminError>` and propagate
failures with `?`. `AdminError` renders a styled, self-contained error page:

| Variant | Status | Page |
| --- | --- | --- |
| `NotFound` | 404 | "Not found" |
| `Forbidden` | 403 | "Forbidden" |
| `Internal(_)` | 500 | "Something went wrong" |

## Meaning is preserved

`From` impls map errors to the matching status, so a domain error keeps its
meaning instead of collapsing to a 500:

```rust
async fn edit(state: &AdminState, id: &str) -> Result<Response, AdminError> {
    let row = fetch(state, id)
        .await?                        // CoreError::NotFound -> 404, DB error -> 500
        .ok_or(AdminError::NotFound)?; // a missing row -> 404
    Ok(render(view(row)))
}
```

`CoreError::NotFound` becomes a 404, `Forbidden`/`Unauthorized` a 403, and only
genuinely unexpected failures (a DB error, a render failure, a panic) become an
`Internal` 500. `AuthError` routes through `CoreError`, so it inherits the same
mapping.

## Internal errors: logged, then masked

An `Internal` error logs its cause through `tracing` and shows the client a
generic page, so the cause reaches the operator's log while nothing internal
leaks to the browser. In **debug mode** the 500 page also shows the cause, for
development; set it in config:

```toml
[app]
debug = true   # off by default; error pages reveal the cause when on
```

Logging is independent of `debug`: the log always records the cause (via
`tracing::error!`), so production, where the page masks, still has it. A stray
panic in any handler is caught and rendered as the same 500, rather than dropping
the connection, and an unmatched admin URL renders the 404 page.

## Validation is a 422

A failed form submission is not an `AdminError`; it re-renders the form inline
with per-field messages (see [Validation](validation.md)) and returns
`422 Unprocessable Entity`. That status is the cross-surface convention for a
validation failure (an API will serialise the same `ErrorBag` as a 422 body).

## Seam for richer rendering

Every error response carries an `ErrorMeta` extension recording its kind. A later
per-surface rendering layer (a shell-wrapped admin page, an HTMX fragment for
partial swaps, or a themed public-web page) can read that marker and re-render
without any handler changing. Until then the standalone page is what ships.
