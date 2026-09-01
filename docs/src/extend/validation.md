# Validation

Every form submission is checked by one rule engine (the same engine an API uses
for its writes). A field carries rules; a failed submission re-renders the form
with a message under each offending field and writes nothing.

## Declaring rules on a form

Rules are builders on a form field:

```rust
use laterite_admin::form::FormField;

let fields = vec![
    FormField::text("email", "Email").required().email().unique(),
    FormField::text("name", "Name").required().max_length(120),
];
```

| Builder | Rule |
| --- | --- |
| `.required()` | non-empty in every mode |
| `.required_on(Mode::Create)` | non-empty on create (or update) only |
| `.min_length(n)` / `.max_length(n)` | length bounds (min is skipped when empty) |
| `.email()` | a valid email address |
| `.unique()` | not already used in this field's column; ignores the edited row on update |

The generic create and edit handlers validate before writing, so a resource with
a `form` gets this automatically.

## The error bag

Validation returns a [`laterite_core::validation::ErrorBag`]. An empty bag means
the submission is valid. The form reads it per field; an API serialises it as a
`422` body shaped `{ "field": ["message", ...] }`.

Call the engine directly with `validate_fields` for the cheap rules alone (no
database round-trip), or `validate` to add the `unique` probe:

```rust
use laterite_core::validation::{validate, FieldRules, Mode, Rule};

let rules = vec![FieldRules::new("email", "Email", vec![Rule::Required, Rule::Email])];
let bag = validate(&db, "users", "id", &rules, &data, Mode::Create, None).await?;
if bag.is_empty() {
    // persist
}
```

`unique` runs a single `EXISTS`-style probe, and only for a field that has a value
and passed its cheap rules, so a blank or malformed value is never also probed.

## Notes

- Email shape is checked by the RFC-compliant `email_address` crate;
  deliverability is confirmed by a confirmation email.
- Rules are runtime and descriptor-driven: they come from a `FormConfig` (later
  YAML), resolved when the form is validated.
