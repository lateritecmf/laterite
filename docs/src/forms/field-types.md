# Writing a Field Type

A field type renders one kind of control on any form or settings page that
names it, and turns what it submits into the value that reaches the database.

## Implement the type

```rust
impl FieldType for SwitchField {
    fn view_key(&self) -> &'static str {
        "switch"
    }

    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        scalar_vm("switch", cx)
    }

    fn render_default(&self, vm: &FieldVm) -> Markup {
        Markup::from_template(&SwitchTmpl { vm }).unwrap_or_default()
    }

    fn to_attr(
        &self,
        field: &SubmittedField<'_>,
        _opts: &ResolvedOptions,
        _mode: Mode,
    ) -> Result<Option<AttrValue>, String> {
        Ok(Some(AttrValue::Bool(stored_is_on(field.value()))))
    }
}
```

Method | Role
--- | ---
`view_key` | The name a descriptor uses. A dotted `vendor.name`; a taken key aborts the boot.
`view_model` | The submitted or saved value as a view-model.
`render_default` | The markup.
`to_attr` | The submitted string as the typed value that reaches the database. `Err` refuses the save.
`option_keys` | The keys the type reads from a field's entry. Any other key is refused by name.
`submitted` | For a type of several controls: what it submitted, gathered into one value. The rules read it, and a refused save is rebuilt from it.
`resolve_options` | Those keys, typed once.

Built in: `text`, `textarea`, `select`, `radio`, `checklist`, `switch`, `date`,
`password`, `reference`, `repeater`.

## Register it

```rust
registry.add_field_type(FieldTypeReg::new(Arc::new(MoneyField)));

FormField::of("price", "Price", "acme.money")
```

One registration covers forms and settings. A descriptor naming an
unregistered type aborts the boot.

## Declare its keys

```yaml
price: { type: acme.money, currency: EUR, precision: 2, span: 1/3 }
```

```rust
fn option_keys(&self, _raw: &serde_json::Value) -> Option<Vec<String>> {
    Some(vec!["currency".into(), "precision".into()])
}
```

A type's keys are written on the field, beside the field's own.

## Meet the contract

Rule | How
--- | ---
Works with no configuration | Every key has a default chosen for the common case: `type: acme.money` alone renders a usable control.
Each default can be turned off | One key per behaviour, named for what it does, read from the field's entry.
Its keys are declared | `option_keys` lists them; a typo is refused by name.
Enter stays inside it | A root marked `data-lat-enter-scope` when the control holds an input of its own. See [JavaScript API](../screens/javascript-api.md#keep-enter-inside-a-widget).
It says what it did | `lat.emit(root, 'acme.money:changed', detail)`; a `before-` event is cancelable.
It can be driven | The island's initialiser returns its controller; `lat.get(el)` finds it.
It is listed | One row per behaviour in your plugin's docs: the default, and the key that changes it. See [Behaviours](../reference/behaviours.md).
Works before scripts load | The markup submits as a plain form; a script island improves it.
