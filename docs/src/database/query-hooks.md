# Query Hooks

A query hook adds conditions to every admin read of an entity: list rows and
their count, the edit form, export and relation sections.

## Write a hook

```rust
use laterite_core::{QueryHook, ScopeCx};
use sea_query::{Alias, Expr, SelectStatement};

struct Published;

impl QueryHook for Published {
    fn name(&self) -> &str {
        "acme.published"
    }

    fn apply(&self, cx: &ScopeCx<'_>, select: &mut SelectStatement) {
        select.and_where(Expr::col((Alias::new(cx.entity), Alias::new("published"))).eq(1));
    }
}
```

## Register it

```rust
Contributions {
    query_hooks: vec![
        QueryHookReg::for_entity("posts", Arc::new(Published)),
        QueryHookReg::all(Arc::new(TenantOnly)),
    ],
    ..Default::default()
}
```

Constructor | Applies to
--- | ---
`QueryHookReg::for_entity(entity, hook)` | One entity
`QueryHookReg::all(hook)` | Every entity a screen reads

A record a hook hides has no edit page.

## Apply hooks in your own query

```rust
let mut select = Query::select();
select.column(Alias::new("id")).from(Alias::new("posts"));
laterite_core::scope::apply(&hooks, &ScopeCx::new("posts", db.backend), &mut select, &[]);
```

The last argument lists hook names to leave out, such as `SoftDelete::NAME`.

## Built-in hooks

Hook | Name | Condition
--- | --- | ---
`SoftDelete` | `soft_delete` | `deleted_at IS NULL`; added by [`soft_delete`](soft-delete.md)
