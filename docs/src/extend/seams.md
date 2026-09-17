# Extending Fields, Writes and Lifecycles

Three seams change how the admin behaves. Each is contributed from a module's
`register` and named by a descriptor.

Scope | Seam
--- | ---
One field on a form | A field type
One entity's storage shape | A persister
Any record, cross-cutting | A listener

## Write a field type

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
`view_model` | The submitted or stored value as a view-model.
`render_default` | The markup.
`to_attr` | The submitted string as the typed value that reaches the database. `Err` refuses the save.

Built in: `text`, `textarea`, `select`, `radio`, `switch`, `date`, `password`,
`reference`, `repeater`.

Register it and name it from a descriptor:

```rust
registry.add_field_type(FieldTypeReg::new(Arc::new(MoneyField)));

FormField::of("price", "Price", "acme.money")
```

One registration covers forms and settings. A descriptor naming an
unregistered type aborts the boot.

## Write a persister

A persister replaces the generic insert and update for one entity.

```rust
#[async_trait]
impl Persister for NodePersister {
    async fn create(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<i64, SaveError>;
    async fn update(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), SaveError>;
    async fn load(&self, cx: &mut SaveCx<'_>, id: &str) -> Result<Option<Record>, SaveError>;
    async fn delete(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), DeleteError>;
}
```

Method | Default
--- | ---
`load` | `None`. The pre-write row, for listeners.
`delete` | Refuses.

`cx` carries the open transaction. Run every statement on it; never open your
own.

```rust
registry.add_persister(PersisterReg::new("rainmill.location", Arc::new(NodePersister)));

FormConfig::new("location_node", "Location", "/locations", "id", fields)
    .persist("rainmill.location")
```

## Write a listener

A listener runs around the write of any record it targets. See
[Model Listeners](model-listeners.md).

```rust
registry.add(ModelListenerReg::for_entity("article", Arc::new(NotifyOnPublish)));
registry.add(ModelListenerReg::all(Arc::new(AuditWriter)));
```

An all-entities listener must not assume a column exists.
`FormConfig::timestamps` opts one entity into the built-in `Timestamps`.
