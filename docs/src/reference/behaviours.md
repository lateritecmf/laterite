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
A repeater | Past 3 fields a row, rows collapse to the line naming them; fewer stay open. | `display: list` or `inline`.
A repeater row | Moves up and down, copies into a new row beneath it, and is removed. | `reorder: false`, `duplicate: false`.
A refused save | The form returns with its errors, values kept (a repeater's rows and a checklist's ticks included), and the cursor in the first field it refused. | `focus: off`.
A new record | Opens with the cursor in its first field. | `focus: off`.
Leaving with changes unsaved | Asks first, with the cursor on Stay. Typing and undoing it is not a change. | `confirm_leave: false`.
A field with a `max_length` rule | The control holds the limit, and a count appears once four fifths of it are used. | `counter: true` or `false`.
A textarea | As tall as its text, from `rows` up to most of the window. | `grow: false`.
A password field | A button shows what was typed. Sending the form hides it again. | `reveal: false`.
A field with `preset` | Follows the named field as it is typed, until edited. | Omit the key.
A select past 10 choices | Searched: a box that filters the choices as you type. | `search: true` or `false`.
A picker or a searched select | Opens on focus with the choice made leading. Down and Up move, Enter chooses, Escape puts the choice back, leaving the box empty clears it. | None.
A field with `trigger` | Shows, hides, enables, disables, empties or fills itself as the watched field changes. A hidden field is left out of the submission. | Omit the key.
A checklist past 10 choices | Offers select all and select none. | `select_all: true` or `false`.
A checklist past 20 choices | Offers a search box. | `search: true` or `false`.
A checklist past 10 choices, in groups | A group with every box ticked, or none, starts closed. A group partly ticked starts open. | `expand: all` or `none`.
A checklist group | Its box ticks the whole group, and shows a dash when some are ticked. | None.
A checklist being searched | Select all, select none and a group's box act on the matches. Escape clears the search. | None.

```yaml
form:
  enter: off
  confirm_leave: false
  fields:
    sku:   { enter: next, span: 1/3 }
    links: { type: repeater, display: list, fields: { url: { input: url } } }
    tags:  { type: checklist, select_all: false, expand: all, options: [{ value: new }] }
```

## Lists

Where | Default | Change it
--- | --- | ---
Search | Asks as you type. | `search: { on_enter: true }`; `search: { enabled: false }`.
Columns and page size | Each operator's choice is remembered. | `per_page` sets the size; `per_page_options` offers a choice of sizes.
A column | Sortable, visible. | `sortable: false`, `invisible: true`.
Export | Off. | `exportable: true`.
Deleting rows | Off. | `deletable: true`; it asks before it deletes.
A row | Opens its record on a click or Enter, when the list has a form. A modified or middle click opens a new tab. | `row_click: none`.
Row boxes | Shift and a click ticks or clears the range from the last box clicked. | None.
Returning to a list | The sort, search and filters used last come back, for the session. Clearing them forgets them. | `remember: false`.
A stored code in a column | Shown as stored. | `labels: [{ value, label }]` shows a label for each value.

```yaml
list:
  search: { on_enter: true, prompt: Search posts }
  per_page_options: [25, 50, 100]
  exportable: true
  row_click: none
  remember: false
  columns:
    status: { labels: [{ value: pub, label: Published }, { value: dr, label: Draft }] }
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
