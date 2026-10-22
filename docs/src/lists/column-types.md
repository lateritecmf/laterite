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

## Register it

```rust
registry.add(ColumnTypeReg::new(Arc::new(Rating)));
```

```yaml
columns:
  rating: { type: acme.rating }
```

A taken key aborts the boot; so does a column naming a key nobody registered.
