# Trees and Reordering

A list keeps its rows in an order of their own, shows them as a tree on a
parent column, or both.

## Reorder rows

```yaml
list:
  reorder: true
```

```yaml
list:
  reorder: { column: position }
```

```rust
ListConfig::new("pages", "Pages", columns).reorder("position")
```

Key | Default | Does
--- | --- | ---
`reorder` | off | `true` keeps the order in `sort_order`; `{ column: position }` names the column. An integer column, not null, default 0.
`reorder.column` | `sort_order` | The column holding each row's position.

The list opens in that order with a grip to drag and Move up / Move down on
every row; a sorted, searched or filtered view shows neither. A has-many
section's `list` takes the same key; see
[Related Records](../forms/relations.md).

## Show rows as a tree

```yaml
list:
  tree: true
```

```yaml
list:
  tree: { parent_key: folder_id, source: acme.folders }
```

```rust
use laterite_admin::list::TreeConfig;

ListConfig::new("folders", "Folders", columns)
    .tree(TreeConfig::new("folder_id").move_to("acme.folders"))
```

Key | Default | Does
--- | --- | ---
`tree` | off | `true` reads the parent from `parent_id`; `{ parent_key, source }` names the column and a picker source.
`tree.parent_key` | `parent_id` | The column holding a row's parent id, null at the top level.
`tree.source` | none | A registered picker source over this entity. With it, every row offers "Move to", which picks the new parent from the source or leaves it empty for the top level. A record never moves under itself.

The list opens on the rows with no parent; a row with children expands in
place, or opens them as a page with scripting off. With `reorder` too, a move
stays among one parent's children. A has-many section ignores `tree`.

## Reach the routes

Path | Method | Does
--- | --- | ---
`/products/reorder` | POST | Moves `id` with `move=up` or `move=down`, or `before=<id>` or `after=<id>`, and shows `page` again; `parent` names the branch under a tree.
`/products/7/move` | GET, POST | The "Move to" form for record `7`, and the move itself.

Both answer an htmx request with the list region and a plain one with the page.
