# Navigation

A resource and a screen each add their own menu entry. A module adds sections
and extra entries through its registry.

```yaml
# descriptors/posts.yaml
menu: { under: acme.content, icon: file-text, order: 2 }
```

```rust
registry.add_nav(NavItem::section("acme.content", "Content").icon("folder").order(10));
registry.add_nav(NavItem::new("acme.reports", "Reports", "/reports").permission("acme.reports"));

Resource::new("/posts", "Posts", list)
    .permission("acme.posts")
    .menu(Placement::new().under("acme.content").icon("file-text").order(2))

ScreenReg::new("/reports", "acme.reports", Arc::new(ReportsScreen))
    .in_menu("Reports")
    .menu(Placement::new().under("acme.content"))
```

`menu` key | Default | Does
--- | --- | ---
`under` | the top bar | The key of the section the entry sits under.
`icon` | none | An icon name from the icon set.
`order` | `0` | Sorts among siblings, lowest first; equal orders sort by label.
`after`, `before` | none | Sits right after or before the sibling with that key, whatever the orders say. A resource's key is `resource:` and its path (`resource:/posts`), a screen's `screen:` and its path, a contributed item's its own.
`menu: false` | | Mounts the routes with no menu entry.

`NavItem` | Does
--- | ---
`NavItem::new(key, label, path)` | An entry leading to `path` under the admin mount.
`NavItem::section(key, label)` | A tab holding entries; it leads to the first one the operator may open.
`.icon(name)`, `.order(n)`, `.after(key)`, `.before(key)`, `.under(key)`, `.permission(code)` | As the `menu` keys above; `permission` hides the entry from an operator without it.

Behaviour | Does
--- | ---
Permission | A resource's entry carries the resource's permission, a screen's its own. An operator sees only the entries they may open; a section with none left is gone.
Sidebar | On a section's pages, its entries show in the context sidebar with the current one marked.
Dashboard and Settings | Always first and last.
The operator's own order | Dragging the tabs in the top bar keeps that order with the operator's preferences, on every device; "Reset the menu order" on Preferences restores the default. Dashboard and Settings stay put.
Keys | A duplicate key, `under` naming a key nobody declared, or `after` and `before` naming no sibling, aborts boot.
