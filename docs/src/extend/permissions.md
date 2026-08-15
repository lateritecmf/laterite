# Permissions

Permissions are dotted strings such as `posts.approve` or `backend.manage_users`.
A descriptor declares the permission a screen requires, and the framework checks
it against the signed-in operator. An operator's permissions come from the roles
assigned to them; a superuser passes every check regardless of the roles held.

## Grants and wildcards

A role grants a set of permission strings. A grant matches in one of three ways:

- An exact grant matches its own string: `posts.approve` grants `posts.approve`.
- A trailing-wildcard grant covers a whole namespace: `posts.*` grants
  `posts.approve` and `posts.tags.create`, but not the bare `posts`.
- The global wildcard `*` grants everything.

Superusers short-circuit the check, so they always pass even with no grants
listed.

## Where permissions apply

A permission string appears on the descriptors that carry a screen, and the
framework enforces it in two places.

### Resource routes

A [`Resource`](https://docs.rs/laterite-admin) carries an optional `permission`.
When set, every route the resource mounts (its list, and its create and edit
forms) is gated: an operator who lacks the grant receives `403 Forbidden`, while
an unauthenticated request is sent to the login screen. A `None` permission
leaves the resource open to any signed-in operator.

```rust
use laterite_admin::Resource;

let pages = Resource {
    base_path: "/admin/pages".to_string(),
    nav_label: "Pages".to_string(),
    list: pages_list_config(),
    form: Some(pages_form_config()),
    permission: Some("acme.manage_pages".to_string()),
};
```

### Settings items

A settings item carries the same optional `permission`. It controls visibility:
an item the operator lacks the grant for is hidden from the settings menu and its
form cannot be opened. Because the framework's built-in Users and Roles are
[settings items linking to resources](settings.md#the-settings-menu-vs-the-main-menu),
their menu entry and their routes are gated together by giving the item and the
resource the same permission.

## Built-in permissions

The framework's own administrative screens are gated by these grants. Assign them
to a role to let an operator manage backend accounts without making them a
superuser:

| Permission | Grants access to |
| --- | --- |
| `backend.manage_users` | The backend users list. |
| `backend.manage_roles` | The roles list and the role editor. |
