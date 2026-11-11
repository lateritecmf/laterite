# Permissions

A permission is a dotted string such as `posts.approve`. A descriptor declares
the one a screen requires; the framework checks it against the operator's
roles.

## Register permissions

```rust
use laterite_admin::{Permission, ROLE_EDITOR};
use laterite_core::t;

registry.add_permission(
    Permission::new("acme.publish_pages", t!("Publish pages"), t!("Content"))
        .roles([ROLE_EDITOR]),
);
```

Registered permissions appear in the role editor under their `group`. Only a
registered permission can be granted.

## Rename a permission

```rust
Permission::new("acme.posts", t!("Manage posts"), t!("Content"))
    .renamed_from(["acme.manage.posts"])
```

At the next start every role and per-user override holding the old code is
rewritten to the new one. Two permissions claiming one old code, or an old
code still registered, abort the start.

## Place a permission in a built-in role

Role | Holds
--- | ---
`ROLE_ADMIN`, Administrator | Every permission that names no role, plus any naming it. The first operator holds it.
`ROLE_EDITOR`, Editor | Only the permissions that name it. Empty until your application registers some.

`Permission::roles` names the roles that hold a permission by default. Naming
none means the administrator alone. Neither role is editable: duplicate one to
make it yours.

## Gate a screen

```rust
use laterite_admin::Resource;

registry.add_resource(
    Resource::new("/pages", "Pages", pages_list_config())
        .form(pages_form_config())
        .permission("acme.manage_pages"),
);
```

Where | Effect
--- | ---
`Resource::permission` | Every route the resource mounts: `403` without the grant, the login screen when signed out. Unset leaves it open to any operator.
`SettingsItem::permission` | Hides the item from the settings menu and refuses its form.
`ScreenReg::new(path, permission, ..)` | Required. Enforced before the handler runs.
`ToolbarButton::require` | The button is not rendered without the grant.
`Shell::allows(code)` | In a module's own handler: whether to show a control or answer `403`.
`Resource::create_permission`, `update_permission`, `delete_permission` | One write each, beside `Resource::permission` (below).

## Gate each write

```yaml
# descriptors/posts.yaml
permission: acme.posts
permissions:
  create: acme.posts.create
  update: acme.posts.update
  delete: acme.posts.delete
```

```rust
Resource::new("/posts", "Posts", list)
    .form(form)
    .permission("acme.posts")
    .create_permission("acme.posts.create")
    .update_permission("acme.posts.update")
    .delete_permission("acme.posts.delete")
```

Key | Asked for by | Without it
--- | --- | ---
`create` | The New form and its save | No New button; `403`
`update` | A save, an editable cell, a move or reorder, a related record's change | The form opens read-only, with no Save; the section has no New; `403`
`delete` | Delete, restore, delete permanently | No row selection or Delete; `403`

An operator needs `permission` and the key's grant. A key left out asks for
`permission` alone. A key naming a permission no module registers
aborts the start.

## Match a grant

Grant | Covers
--- | ---
`posts.approve` | Exactly `posts.approve`.
`posts.*` | `posts.approve`, `posts.tags.create`; not the bare `posts`.
`*` | Everything.

A superuser passes every check.

## Resolve for one operator

1. A superuser: allowed.
2. The operator's own override for that permission: **Deny** refuses, **Allow** grants.
3. Otherwise the roles decide.

Roles and overrides are both set per user on the **Administrators** screen;
**Inherit** is the default.

## Grant a built-in permission

Permission | Grants
--- | ---
`backend.manage_users` | The backend users list and per-user overrides.
`backend.manage_roles` | The roles list and the role editor.
`backend.manage_groups` | The groups list, a group's form and its members.
`backend.manage_plugins` | The plugins screen.
`backend.view_audit_log` | The audit log.
`backend.view_access_log` | The access log.
`backend.view_request_log` | The request log.
