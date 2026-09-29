# Behaviours

What the admin does with no configuration, and the key that changes it.

## Forms

Where | Default | Change it
--- | --- | ---
A single-line field | Enter submits the form. | `enter: off` on the form; `enter: off` or `enter: next` on a field.
A textarea | Enter is a newline; Cmd/Ctrl+Enter submits. | None.
A picker or a repeater row | Enter acts inside it. | `data-lat-enter-scope` and `data-lat-enter-target` on a widget of your own.
A field's width | The whole row. | `span: 1/2`, `1/3`, `2/3`, `1/4`, `3/4`; `break: true`.
A narrow window | Under 1100px a quarter is a half; under 768px every field is full. | None.
A key the type does not read | Refused by name. | None.
A repeater | Every row open. | `display: list` collapses each row to the line naming it.
A refused save | The form returns with its errors, values kept. | None.

```yaml
form:
  enter: off
  fields:
    sku:   { enter: next, span: 1/3 }
    links: { type: repeater, display: list, fields: { url: { input: url } } }
```

## Lists

Where | Default | Change it
--- | --- | ---
Search | Asks as you type. | `search: { on_enter: true }`; `search: { enabled: false }`.
Columns and page size | Each operator's choice is remembered. | `per_page` sets the size; `per_page_options` offers a choice of sizes.
A column | Sortable, visible. | `sortable: false`, `invisible: true`.
Export | Off. | `exportable: true`.
Deleting rows | Off. | `deletable: true`; it asks before it deletes.

```yaml
list:
  search: { on_enter: true, prompt: Search posts }
  per_page_options: [25, 50, 100]
  exportable: true
```

## Messages and dialogs

Where | Default | Change it
--- | --- | ---
A success or info message | Leaves after five seconds. | `session.push_flash_sticky(..)` keeps it until dismissed.
An error message | Stays until dismissed. | None.
A control with `data-lat-confirm` | Asks before it acts. | Omit the attribute.
A menu | Closes on a click outside, a chosen link, or Escape. | `data-lat-keep-open`.

## Accounts and sessions

Where | Default | Change it
--- | --- | ---
A new password | At least 8 characters. | `[auth.password_policy] min_length`.
A generated password | Temporary: every screen leads to Preferences until it is replaced. | `lat admin create` without `--generate` sets a password that is kept.
Failed sign-ins | 5 for a username from one address, 20 from one address, in 15 minutes. | `[auth] max_failures`, `max_failures_per_address`, `failure_window_secs`.
A session | Ends after 2 hours idle, or 12 hours. | `[auth] session_idle_timeout_secs`, `session_absolute_timeout_secs`.
Expired sessions | Removed at boot and hourly. | None; `lat admin purge` runs it on demand.

```toml
[auth]
max_failures = 5
session_idle_timeout_secs = 7200

[auth.password_policy]
min_length = 12
```
