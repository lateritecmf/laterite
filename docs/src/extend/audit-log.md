# Audit Log

Every administrative change is recorded in an append-only log. Handlers do not
opt in.

## What an entry holds

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

## View it

**Settings → System → Audit Log**, newest first, read-only. Gated by
`backend.view_audit_log`; superusers hold it.

## The access log

**Settings → System → Access Log**, newest first, read-only, gated by
`backend.view_access_log`. Sign-in events, with the address and client they
came from:

Event | Recorded when
--- | ---
`login_success` | A sign-in succeeded.
`login_failure` | A password was wrong, or the username unknown.
`locked_out` | An attempt was refused by the lockout.
`logout` | An operator signed out.
`password_changed` | An operator changed their own password.

## Your own resources

A `Resource` with a form is audited as `backend.<entity>.create` and
`backend.<entity>.update`, attributed to the signed-in operator, through a
[model listener](model-listeners.md).

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

The audit write runs after the change commits. A failed audit write is logged;
the operator's action stands.
