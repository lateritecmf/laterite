# Logs

The admin keeps three read-only logs under **Settings → System**: the audit
log of administrative changes, the access log of sign-ins, and the request log
of URLs it could not serve. Each is gated by its own permission and exportable.

## Read the audit log

Every administrative change is recorded in an append-only log; handlers do not
opt in. Gated by `backend.view_audit_log`; superusers hold it.

Field | Value
--- | ---
When | The time of the change.
Operator | The username, kept on the entry after the account is removed. A system change records the process name.
Action | A dot-keyed name: `backend.role.update`, `backend.plugin.disable`.
Target | A type, id and name, when the action has one.

Change | Recorded
--- | ---
A role created or edited | Yes
A user's permissions changed | Yes
A password changed | Yes, as `backend.user.password_change`, never the password
An operator created from setup or `lat admin create` | Yes, as `backend.user.create`, credited to the process
A plugin enabled or disabled | Yes
A settings model saved | Yes, without the contents
A record created or edited through a resource form | Yes
An operator's own preferences | No

## Read the access log

Every sign-in, failure, lockout, sign-out and self-service password change,
with the address and client it came from. Gated by `backend.view_access_log`.

Column | Holds
--- | ---
When | The time of the event.
Username | The username attempted, searchable.
Event | One of the events below; a filter narrows to one.
Address | The IP address, searchable.
Client | The user agent; hidden until turned on in the list setup.

Event | Recorded when
--- | ---
`login_success` | A sign-in succeeded.
`login_failure` | A password was wrong, or the username unknown.
`locked_out` | An attempt was refused by the lockout.
`logout` | An operator signed out.
`password_changed` | An operator changed their own password.

## Read the request log

Every URL the admin could not serve is counted, once per method, path and
status, with the first and last time it was seen. Gated by
`backend.view_request_log`.

```yaml
# descriptors/request-log.yaml, built in
entity: backend_request_log
path: /request-log
permission: backend.view_request_log
```

Column | Holds
--- | ---
Last seen, First seen | When the URL was last and first requested.
Method, Path | The request; the path is kept to 512 characters, without its query string.
Status | `404`.
Hits | How many times.

Rows are selected and deleted like any list's.

## Write audit entries from your own resources

A `Resource` with a form is audited as `backend.<entity>.create` and
`backend.<entity>.update`, attributed to the signed-in operator, through a
[model listener](../database/model-listeners.md).

A screen of your own records an entry with the builder:

```rust
use laterite_auth::AuditEntry;
use laterite_core::Actor;

let actor = Actor::user(operator.id, &operator.username);
auth.record_audit(
    AuditEntry::new(&actor, "acme.posts.publish").target("post", &post_id, Some(&post.title)),
)
.await?;
```

Builder | Description
--- | ---
`AuditEntry::new(actor, action)` | Who, and a dot-keyed action under your module's id.
`.target(type, id, name)` | What the action touched; `name` may be `None`.

The audit write runs after the change commits. A failed audit write is logged;
the operator's action stands.
