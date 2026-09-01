# Sessions and CSRF

The admin signs an operator in with a session cookie. Alongside the identity,
each session carries a small typed blob for surface state: a CSRF token and flash
messages. It is read in the same query that resolves the session, so using it
costs no extra round-trip, and written back only when a request changed it.

## CSRF protection

Every state-changing admin request (`POST`, `PUT`, `PATCH`, `DELETE`) is checked
by a default-deny layer on the admin mount. Two gates must pass:

1. **Origin.** The request must come from our own origin, confirmed by
   `Sec-Fetch-Site: same-origin` or a matching `Origin` header. Set `app.url` in
   production so the expected origin is exact; in development it falls back to the
   request `Host`.
2. **Token.** A per-session token must arrive in the `_csrf` form field or the
   `X-CSRF-Token` header, and match the session's.

Safe methods (`GET`, `HEAD`, `OPTIONS`, and the `QUERY` method) are exempt.

You get this for free:

- **Descriptor forms** rendered by the framework already include the hidden token
  field.
- **HTMX requests** carry the token as a header, injected once on the page body.
- The login and first-run setup screens are the only token-less mutations (no
  session exists yet), so the origin gate is their sole defense.

A **hand-written form** in a template includes the token with one line:

```html
<form method="post" action="{{ action }}">
  {% include "_csrf.html" %}
  ...
</form>
```

A missing or wrong token, or a foreign origin, renders a `403` "Request blocked"
page telling the operator to reload and try again.

## Flash messages

A handler queues a message for the next full-page render (the redirect-after-POST
target) through the session handle:

```rust
use laterite_admin::{FlashLevel, SessionHandle};

async fn save(Extension(session): Extension<SessionHandle>, /* ... */) -> Response {
    // ... persist ...
    session.push_flash(FlashLevel::Success, "Settings saved.");
    Redirect::to("/admin/settings").into_response()
}
```

The shell renders and clears queued messages on the next full page, so a redirect
carries its confirmation across. Levels are `Success`, `Error`, and `Info`.

## Notes

- A fresh login always mints a new session and a new token; the framework never
  adopts a token presented by the client.
- The blob is versioned and size-capped; a corrupt blob degrades to an empty
  session, never an authentication failure.
- The token layer has a pluggable source: the admin uses a session-bound
  synchronizer token; a stateless signed token for public forms attaches later
  without changing handlers.
