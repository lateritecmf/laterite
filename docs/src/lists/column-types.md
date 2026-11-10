# Writing a Column Type

A column type renders one kind of cell on any list that names it.

## Implement the type

```rust
use laterite_admin::html::Markup;
use laterite_admin::list::{CellCx, CellVm, ColumnType, ColumnTypeReg};

struct Rating;

impl ColumnType for Rating {
    fn view_key(&self) -> &'static str { "acme.rating" }
    fn view_model(&self, cx: &CellCx<'_>) -> CellVm { /* ... */ }
    fn render_default(&self, vm: &CellVm) -> Markup { /* ... */ }
}
```

Method | Role
--- | ---
`view_key` | The name a column's `type` uses. A dotted `vendor.name`.
`view_model` | The cell's value as a view-model.
`render_default` | The markup.
`option_keys` | The keys the type reads from its column entry; any other key is refused at boot.

`CellCx` | Gives
--- | ---
`value` | The stored value, as text.
`tz`, `locale` | The display timezone and locale.
`options` | The column's entry, for the type's own keys.
`option_str(key)` | One of those keys, as text.
`field(column)` | Another value in the row; `field("id")` is the record's id.
`icon(name)` | Inline SVG for an icon from the set.
`base()` | The admin mount, for an address inside the admin.
`CellCx::new(value, tz, locale)` | A context for a test, with `with_options`, `with_row`, `with_icons`, `with_base`.

## Register it

```rust
registry.add(ColumnTypeReg::new(Arc::new(Rating)));
```

```yaml
columns:
  rating: { type: acme.rating }
```

A taken key aborts the boot; so does a column naming a key nobody registered.
