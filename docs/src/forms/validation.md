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

```yaml
fields:
  email: { rules: [required, email, unique] }
  name: { rules: [required, { max_length: 120 }] }
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

## Register a rule

```rust
use laterite_core::validation::{RuleCx, ValidationRule, ValidationRuleReg};
use laterite_core::Text;

struct DiffersFrom;

#[laterite_core::strata::async_trait]
impl ValidationRule for DiffersFrom {
    async fn check(&self, cx: &RuleCx<'_>, value: &str) -> Result<(), Text> {
        let other = cx.args.first().and_then(|name| cx.data.get(name));
        if other.is_some_and(|o| o.trim() == value) {
            return Err(Text::new("{label} must differ.").arg("label", Text::dynamic(cx.label)));
        }
        Ok(())
    }
}

Contributions {
    validation_rules: vec![ValidationRuleReg::new("acme.differs_from", Arc::new(DiffersFrom))],
    ..Default::default()
}
```

```yaml
fields:
  code: { rules: [required, acme.uppercase, { acme.differs_from: name }] }
```

`RuleCx` field | Holds
--- | ---
`field`, `label` | The field being checked and its label
`args` | The descriptor's arguments as text: `{ acme.between: [1, 10] }` gives `["1", "10"]`
`data` | Every submitted value, by field name
`mode` | `Mode::Create` or `Mode::Update`
`db`, `table`, `edited_id` | The database, the form's table, the record being updated

A rule runs after the built-in rules pass for that field, and skips an empty
value unless `implicit()` returns `true`. A registered name is dotted
(`vendor.rule`). A field naming a rule no module registered fails boot.

## Read what a submission answers

Outcome | Response
--- | ---
Refused | `422`, the form re-rendered with its values. An htmx submit swaps the form; a plain one gets the whole page.
Saved | A flash, then the list: `HX-Redirect` for htmx, `303` otherwise.
API | `422` with `{ "field": ["message", ...] }`.
