# Sessions and CSRF

A session cookie signs an operator in. Alongside the identity it carries a
CSRF token and flash messages.

## CSRF

Every `POST`, `PUT`, `PATCH` and `DELETE` on the admin mount passes two gates.
`GET`, `HEAD`, `OPTIONS` and `QUERY` are exempt.

Gate | Passes when
--- | ---
Origin | `Sec-Fetch-Site: same-origin`, or `Origin` matches `app.url`. In development the request `Host` stands in.
Token | The session's token arrives in the `_csrf` field or the `X-CSRF-Token` header.

Descriptor forms and htmx requests carry the token already. A hand-written
form includes it:

```html
<form method="post" action="{{ action }}">
  {% include "_csrf.html" %}
  ...
</form>
```

A failed gate renders a `403` "Request blocked" page. Login and first-run
setup have no session and pass on origin alone.

## Flash a message

```rust
use laterite_admin::{FlashLevel, SessionHandle};

async fn save(Extension(session): Extension<SessionHandle>, /* ... */) -> Response {
    // ... persist ...
    session.push_flash(FlashLevel::Success, "Settings saved.");
    Redirect::to("/admin/settings").into_response()
}
```

The next full page renders and clears it, across a redirect. Levels:
`Success`, `Error`, `Info`.

A fresh login mints a new session and token. A corrupt session blob degrades
to an empty session, never a sign-out.
