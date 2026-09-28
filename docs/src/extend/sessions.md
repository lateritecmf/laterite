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
`Success`, `Error`, `Info`. `push_flash_sticky` keeps the message until
dismissed.

## Change a password

```rust
use laterite_core::Actor;

auth.change_password(
    user.id,
    &new_password,
    Some(&session_token),
    &Actor::user(user.id, &user.username),
)
.await?;
```

Argument | Value
--- | ---
`user_id` | The account whose password changes.
`new_password` | Refused under `password_policy.min_length` (default 8).
`keep_token` | The session to keep signed in, or `None` to end all of them.
`actor` | Who made the change, recorded on the audit log. `Actor::system("lat admin reset-password")` for a process.

`AuthService::password_changed_at(user_id)` returns when it last changed, or
`None` if never recorded.

An operator changes their own under **Preferences → Password**, which calls:

```rust
auth.change_own_password(user.id, &current, &new_password, &session_token, &ctx)
    .await?;
```

Error | Returned when
--- | ---
`AuthError::InvalidCredentials` | `current` does not match. Counts as a failed sign-in.
`AuthError::TooManyAttempts` | The account is locked out.
`AuthError::Refused` | The new password is under `password_policy.min_length`.

A change through this call is also on the access log as `password_changed`,
with the address it came from.

## Why a session ended

Event | The signed-out device sees
--- | ---
`AuthService::change_password` | "Your password was changed"; every other session and stay-signed-in device ends.
Deactivation | "This account was deactivated."
Sign out one device, or every other device | "You were signed out from another device."
Inactivity or the absolute ceiling | "Your session ended after a period of inactivity."

The login URL carries `?ended=<code>`, from a fixed set. An
htmx request gets `HX-Redirect` with no body.

A fresh login mints a new session and token. A corrupt session blob degrades
to an empty session, never a sign-out.
