# Validation

A form field carries rules. A failed submission re-renders the form with a
message under each field and writes nothing.

## Declare rules

```rust
use laterite_admin::form::FormField;

let fields = vec![
    FormField::text("email", "Email").required().email().unique(),
    FormField::text("name", "Name").required().max_length(120),
];
```

Builder | Rule
--- | ---
`.required()` | Non-empty in every mode.
`.required_on(Mode::Create)` | Non-empty on create, or on update, only.
`.min_length(n)` / `.max_length(n)` | Length bounds. `min` is skipped when empty.
`.email()` | A valid email address.
`.unique()` | Not already used in the column. Ignores the edited row on update.

The generic create and edit handlers validate before writing.

## Call the engine

```rust
use laterite_core::validation::{validate, FieldRules, Mode, Rule};

let rules = vec![FieldRules::new("email", "Email", vec![Rule::Required, Rule::Email])];
let bag = validate(&db, "users", "id", &rules, &data, Mode::Create, None).await?;
if bag.is_empty() {
    // persist
}
```

`validate_fields` runs the rules without the database; `validate` adds the
`unique` probe. The result is an `ErrorBag`; empty means valid.

## What a submission answers

Outcome | Response
--- | ---
Refused | `422`, the form re-rendered with its values. An htmx submit swaps the form; a plain one gets the whole page.
Saved | A flash, then the list: `HX-Redirect` for htmx, `303` otherwise.
API | `422` with `{ "field": ["message", ...] }`.
