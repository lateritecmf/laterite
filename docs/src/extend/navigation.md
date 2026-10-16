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
`slot` | `application` | The band the entry belongs to (see below).
`order` | `0` | Sorts among siblings of the band, lowest first; equal orders sort by label.
`after`, `before` | none | Sits right after or before the sibling with that key, whatever the bands say. A resource's key is `resource:` and its path (`resource:/posts`), a screen's `screen:` and its path, a contributed item's its own.
`menu: false` | | Mounts the routes with no menu entry.

Slot | Value | Holds
--- | --- | ---
`home` | 0 | The dashboard.
`content` | 100 | What the site publishes.
`media` | 200 | The media library.
`application` | 500 | The application's own screens: the default.
`people` | 700 | Accounts and roles.
`tools` | 800 | Reports, logs and tools.
`system` | 900 | Settings.
`last` | `i32::MAX` | After everything.

```toml
[backend.order]
"backend.nav" = ["dashboard", "acme.shop", "media", "settings"]
"backend.nav.acme.content" = ["resource:/pages", "resource:/posts"]
"backend.settings" = ["backend.branding", "web.themes"]
```

Hierarchy | Keys | Pinned
--- | --- | ---
`backend.nav` | The top bar's entries. | `dashboard` first, `settings` last.
`backend.nav.<section>` | A section's entries. | none
`backend.settings` | Settings items by code. | none

A key left out keeps its arrangement after the listed ones; a key that is no
entry is skipped. An operator's own drag of the top bar sits on top of the
deployment's sequence.

```toml
[backend.menu]
"backend.access_log" = { in = "backend.nav", slot = "tools" }
"resource:/orders" = { in = "backend.settings", category = "Shop", order = 5 }
"screen:/reports" = { in = "backend.nav.acme.content" }
```

`[backend.menu]` key | Meaning
--- | ---
The entry | A menu key (`resource:/orders`, `screen:/reports`, a contributed item's key) or a settings item's code.
`in` | `backend.nav`, `backend.nav.<section>` or `backend.settings`.
`slot`, `order` | The band and order there; the entry's own when left out.
`category` | The settings heading; the band's name when left out.

A moved entry keeps its label, icon, permission and target: a resource moved
into settings opens at its own path with the settings sidebar beside it, and
a settings item moved into the top bar is a tab. A section cannot move into
settings. The boot log prints every hierarchy's arrangement, one line each.

An operator arranges the two hierarchies for themselves on top of the
deployment's: dragging the tabs in the top bar and the entries in the
settings list, and choosing on Preferences, under Menu, where each entry is
shown. Reset the menu there puts everything back.

Preference | Holds
--- | ---
`menu.order` | The top bar's keys in the operator's order.
`menu.order.backend.settings` | The settings index's codes in the operator's order.
`menu.moves` | `{"<key>": "nav" | "settings"}` for each entry the operator moved.

A hierarchy a plugin declares takes operator preferences only when it says
`operator_adjustable()`.

```rust
use laterite_core::arrange::{arrange, Arranged, Hierarchy, Overrides, Policy, Position, slot};

let widgets = arrange(
    &Hierarchy::new("acme.dashboard").policy(Policy::Fixed),
    widgets,
    Overrides { deployment: config.order.get("acme.dashboard").map(Vec::as_slice), operator: None },
)?;
```

`Hierarchy` builder | Does
--- | ---
`new(key)` | The key `[backend.order]` names.
`policy(Policy::Fixed)` | Renders the declared arrangement; overrides are logged and ignored. `Adjustable` is the default.
`operator_adjustable()` | Lets an operator's preference rearrange it too.
`pin_first(key)`, `pin_last(key)` | Keys that hold the ends whatever anyone says.

`NavItem` | Does
--- | ---
`NavItem::new(key, label, path)` | An entry leading to `path` under the admin mount.
`NavItem::section(key, label)` | A tab holding entries; it leads to the first one the operator may open.
`.icon(name)`, `.order(n)`, `.after(key)`, `.before(key)`, `.under(key)`, `.permission(code)` | As the `menu` keys above; `permission` hides the entry from an operator without it.

Behaviour | Does
--- | ---
Permission | A resource's entry carries the resource's permission, a screen's its own. An operator sees only the entries they may open; a section with none left is gone.
Sidebar | On a section's pages, its entries show in the context sidebar with the current one marked.
Breadcrumbs | On a section's pages and on settings pages: the tab, then the entry, above the page. A tab's own page shows none.
Dashboard and Settings | Always first and last.
The operator's own order | Dragging the tabs in the top bar keeps that order with the operator's preferences, on every device; "Reset the menu order" on Preferences restores the default. Dashboard and Settings stay put.
Keys | A duplicate key, `under` naming a key nobody declared, or `after` and `before` naming no sibling, aborts boot.

## Command palette

```yaml
# descriptors/posts.yaml
palette: acme.posttitles
```

```rust
Resource::new("/posts", "Posts", list).palette("acme.posttitles")
```

Key | Does
--- | ---
`palette` | A registered picker source. The palette finds this resource's records through it and opens each at its edit page. Absent keeps the records out.

Behaviour | Does
--- | ---
Opening | The search button in the top bar, or Ctrl+K (Cmd+K). Without scripting the button is `/palette`, the same finder as a page.
Finding | Menu entries and settings the operator may open, by a case-insensitive substring; then records from every resource with a `palette` source, up to eight each. A blank query lists the pages alone.
Keys | Up and Down move, Enter goes, Escape closes.
