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

## Built-in roles

Every deployment starts with two roles the framework owns and rewrites at each
boot from the registry.

Role | Holds
--- | ---
`ROLE_ADMIN`, Administrator | Every permission that names no role, plus any naming it. The first operator holds it.
`ROLE_EDITOR`, Editor | Only the permissions that name it. Empty until your application registers some.

`Permission::roles` names the roles that hold a permission by default. Naming
none means the administrator alone, so a permission you forget to place never
reaches editors. Neither role is editable: duplicate one to make it yours.

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

## How a grant matches

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

Roles and overrides are both set per user on the **Backend Users** screen;
**Inherit** is the default. An operator can change only permissions they hold
themselves, only roles granting no more than they hold, and never their own
roles.

## Built-in permissions

Permission | Grants
--- | ---
`backend.manage_users` | The backend users list and per-user overrides.
`backend.manage_roles` | The roles list and the role editor.
`backend.manage_plugins` | The plugins screen.
`backend.view_audit_log` | The audit log.
