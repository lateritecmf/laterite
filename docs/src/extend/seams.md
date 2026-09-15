# Extending Fields, Writes and Lifecycles

Three seams let a module change how the admin behaves without a framework
change. Which one you want depends on the scope of what you are changing:

| Scope | Seam |
|---|---|
| One **field** on a form | a field type |
| One **entity's** storage shape | a persister |
| **Any record**, cross-cutting | a listener |

All three are contributed from a module's `register`, and all three are
registered by a key a descriptor names. None of them needs a generator: write
`impl Trait for YourType {}` and the compiler lists every method you still owe
it, with its signature.

## A field type: one kind of input

A `FieldType` renders one form field and reads it back. The framework ships
`text`, `textarea`, `select`, `radio`, `switch`, `date`, `password`, `reference`
and `repeater`. Write one when the input you need does not exist: a colour
picker, a money amount, a map point.

The whole of the built-in switch, as an example of how small one can be:

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
        // Absent means unchecked, so this always writes a value: leaving the
        // attribute out would keep the old one and the box would never clear.
        Ok(Some(AttrValue::Bool(stored_is_on(field.value()))))
    }
}
```

`to_attr` is the part that matters. It is the **typed-save contract**: the
submitted string becomes the typed value that reaches the database. A money field
receives `"12.50"` and stores minor units; a switch that received nothing stores
`false` rather than leaving the old value in place.

A descriptor names a type by its key:

```rust
FormField::of("price", "Price", "acme.money")
```

The same type then works on **every** surface: a form field and a settings field
are the same descriptor, so one registration covers both. A descriptor naming a
type nobody registered aborts boot, naming the field and the type.

Contribute it from your module's `register`:

```rust
registry.add_field_type(FieldTypeReg::new(Arc::new(MoneyField)));
```

The key is the type's own `view_key`, so a registration cannot disagree with what
it registers. Use a dotted `vendor.name`: a name that collides with a built-in or
another module aborts boot rather than silently winning.

## A persister: one entity's write path

A `Persister` replaces the generic insert and update for one entity. Reach for
it when a write is not a single statement and a half-finished one would be
corrupt.

```rust
#[async_trait]
impl Persister for NodePersister {
    async fn create(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<i64, SaveError>;
    async fn update(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), SaveError>;

    /// The pre-write row, so a listener can see what changed. Default `None`.
    async fn load(&self, cx: &mut SaveCx<'_>, id: &str) -> Result<Option<Record>, SaveError>;

    /// Default refuses: deleting is a decision an entity opts into.
    async fn delete(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), DeleteError>;
}
```

The worked example is the place tree in `rainmill.location`. Saving a node also
rewrites its ancestor-closure rows, and a node whose closure is half-written is
broken in a way no later write repairs.

**`cx` carries the pipeline's open transaction.** Your extra statements run on
the same connection, so they commit or roll back with the row itself. Never open
your own.

Name it from the descriptor:

```rust
registry.add_persister(PersisterReg::new("rainmill.location", Arc::new(NodePersister)));

FormConfig::new("location_node", "Location", "/locations", "id", fields)
    .persist("rainmill.location")
```

## A listener: anything that reacts to a write

A `ModelListener` runs around the write of any record it targets. The
framework's own timestamps and audit log are listeners, not special cases:

```rust
#[async_trait]
impl ModelListener for Timestamps {
    async fn before_save(
        &self,
        _cx: &mut SaveCx<'_>,
        rec: &mut Record,
        op: Op,
    ) -> Result<(), ErrorBag> {
        let now = Utc::now();
        if op == Op::Create && !rec.contains(CREATED_AT) {
            rec.set(CREATED_AT, now);
        }
        rec.set(UPDATED_AT, now);
        Ok(())
    }
}
```

Four hooks, all optional:

- `before_save` runs **inside the transaction**, so it sees in-flight state and
  can change the record before it is written. Returning an `ErrorBag` refuses the
  write, rolls it back, and puts those messages on the operator's fields.
- `after_save` runs **outside**, once the write is committed. This is where a
  notification or a cache invalidation belongs.
- `before_delete` can refuse with a reason; `after_delete` runs after.

Target it at one entity or at all of them:

```rust
registry.add(ModelListenerReg::for_entity("article", Arc::new(NotifyOnPublish)));
registry.add(ModelListenerReg::all(Arc::new(AuditWriter)));
```

**One rule worth taking seriously:** a transaction stays open across
`before_save`, so a listener must not call a slow service. Send the email from
`after_save`, or queue it.

An all-entities listener runs for every table, so it must not assume columns
exist. Timestamps is opted into per form (`FormConfig::timestamps`) for exactly
that reason: appending `created_at` to an entity without the column would break
every write to it.

## Choosing between them

If you find yourself writing a persister to set one field, you want a listener.
If you find yourself writing a listener that only ever fires for one entity's one
column, you want a field type. The three-way split is per-field, per-record,
per-storage-shape, and picking the narrowest one that fits keeps the other two
out of your way.
