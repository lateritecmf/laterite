# Sessions and CSRF

A session cookie signs an operator in. Alongside the identity it carries a
CSRF token, flash messages and a data blob.

## Send the CSRF token

Every `POST`, `PUT`, `PATCH` and `DELETE` on the admin mount carries the
session's token, in the `_csrf` field or the `X-CSRF-Token` header. Descriptor
forms and htmx requests carry it already. A hand-written form includes it:

```html
<form method="post" action="{{ action }}">
  {% include "_csrf.html" %}
  ...
</form>
```

A scripted request sends the header; a template exposes the token as
`shell.csrf_token`:

```html
<button type="button" data-acme-publish data-csrf="{{ shell.csrf_token }}">Publish</button>
```

```js
document.addEventListener('click', function (e) {
  var button = e.target.closest('[data-acme-publish]');
  if (!button) return;
  fetch(button.dataset.action, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'X-CSRF-Token': button.dataset.csrf }
  });
});
```

A request without the token renders a `403` "Request blocked" page. Login and
first-run setup have no session and need no token.

## Read an upload

```rust
use laterite_admin::VerifiedUpload;

async fn import(mut upload: VerifiedUpload) -> Result<Response, AdminError> {
    while let Some(field) = upload.next_field().await? {
        // ... stream the field ...
    }
    Ok(Redirect::to("/admin/import").into_response())
}
```

An upload's form puts the token in its first field; `VerifiedUpload` checks
it while reading and offers the remaining fields as `Multipart` does. Take an
upload with it, never with a bare multipart parser.

## Load scripts as assets

Every admin response carries these; a handler that sets one of its own keeps
its value. Inline scripts and handlers are refused: a screen binds its
behaviour from a script file and a `data-` attribute.

Header | Value
--- | ---
`Content-Security-Policy` | Scripts and fonts from the admin's own assets; inline styles allowed; images and media also from `https:`
`X-Content-Type-Options` | `nosniff`
`X-Frame-Options` | `SAMEORIGIN`
`Referrer-Policy` | `same-origin`
`Permissions-Policy` | `camera=(), microphone=(), geolocation=(), payment=()`
`Cross-Origin-Opener-Policy` | `same-origin`
`Strict-Transport-Security` | `max-age=31536000; includeSubDomains`, with `secure_cookie = true`

```html
<button type="button" data-acme-refresh>Refresh</button>
```

```js
document.addEventListener('click', function (e) {
  if (e.target.closest('[data-acme-refresh]')) location.reload();
});
```

## Flash a message

```rust
use laterite_admin::{FlashLevel, SessionHandle};
use laterite_core::t;

async fn save(Extension(session): Extension<SessionHandle>, /* ... */) -> Response {
    // ... persist ...
    session.push_flash(FlashLevel::Success, t!("Settings saved."));
    Redirect::to("/admin/settings").into_response()
}
```

Call | Description
--- | ---
`push_flash(level, text)` | Shown on the next full page, across a redirect, then cleared.
`push_flash_sticky(level, text)` | Shown until dismissed.
`FlashLevel` | `Success`, `Error`, `Info`.

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

An operator's own change, from **Preferences → Password**, goes through:

```rust
auth.change_own_password(user.id, &current, &new_password, &session_token, &ctx)
    .await?;
```

Error | Returned when
--- | ---
`AuthError::InvalidCredentials` | `current` does not match. Counts as a failed sign-in.
`AuthError::TooManyAttempts` | The account is locked out.
`AuthError::Refused` | The new password is under `password_policy.min_length`.

A change through this call is also on the access log as `password_changed`.

A password made for an operator is temporary:

```rust
auth.require_password_change(user.id).await?;
```

Every admin request then redirects to **Preferences → Password** until the
operator sets their own. `lat admin create --generate` and
`lat admin reset-password --generate` mark the account the same way.

## Purge expired sessions

Expired sessions and stay-signed-in credentials are removed for you, and on
demand:

```bash
lat admin purge
```

```rust
let purged = auth.purge_expired().await?;
println!("{} sessions, {} credentials", purged.sessions, purged.remember_tokens);
```

## Read the sign-out notice

A redirect to the login screen carries `?ended=<code>` when the session was
ended for a reason the login page can show; an htmx request gets
`HX-Redirect` with no body.

Code | Shown when
--- | ---
`expired` | Inactivity, or the session's ceiling.
`password_changed` | `AuthService::change_password` ended every other session.
`deactivated` | The account was deactivated.
`signed_out_elsewhere` | One device, or every other device, was signed out.
