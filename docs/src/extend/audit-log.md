# Audit Log

Every administrative change is recorded in an append-only log. Handlers do not
opt in.

## What an entry holds

Field | Value
--- | ---
When | The time of the change.
Operator | The username, kept on the entry after the account is removed. A system change records the process name.
Action | A dot-keyed name: `backend.role.update`, `backend.plugin.disable`.
Target | A type and id, when the action has one.

Change | Recorded
--- | ---
A role created or edited | Yes
A user's permissions changed | Yes
A plugin enabled or disabled | Yes
A settings model saved | Yes, without the contents
A record created or edited through a resource form | Yes
An operator's own preferences | No

## View it

**Settings → System → Audit Log**, newest first, read-only. Gated by
`backend.view_audit_log`; superusers hold it.

## Your own resources

A `Resource` with a form is audited as `backend.<entity>.create` and
`backend.<entity>.update`, attributed to the signed-in operator, through a
[model listener](model-listeners.md).

The audit write runs after the change commits. A failed audit write is logged;
the operator's action stands.
