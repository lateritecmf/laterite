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
| `backend.manage_plugins` | The plugins screen (enable and disable installed plugins). |

## Roles and per-user overrides

Permissions resolve in two layers. **Roles** set the base: an operator holds
every permission granted by any role assigned to them. A **per-user override**
then refines that base for one operator, and takes precedence over their roles.

For a given permission, an operator is allowed it when:

1. they are a superuser (always allowed), otherwise
2. their override for that permission decides: an explicit **deny** refuses it and
   an explicit **allow** grants it, otherwise
3. the role grants decide.

A denial always wins over an allow. Overrides are exact permission codes, so a
denial can carve a single permission out of a wildcard role grant.

## Assigning permissions to a role

The **Roles** screen (under Settings, gated by `backend.manage_roles`) is the
role permission editor. Editing a role shows every registered permission as a
checkbox, grouped under its heading, ticked for the permissions the role already
holds. Saving stores the ticked set on the role, and every operator with that
role gains those grants. Only registered permissions can be ticked, so a role
never carries a permission the deployment has not declared.

## Overriding permissions for one user

The **Backend Users** screen (gated by `backend.manage_users`) edits a single
user's overrides. Each registered permission has a three-state control:

- **Allow**: grant it to this user regardless of their roles.
- **Inherit** (the default): defer to the roles.
- **Deny**: refuse it to this user even if a role grants it.

Two rules keep the screen safe:

- A **superuser** already holds every permission, so the screen shows a note
  rather than the controls for them.
- An operator can only change permissions they hold themselves. Controls for
  permissions they lack are shown locked, and saving never alters those, so the
  screen cannot grant access beyond the editing operator's own.

## Registering permissions

Register the permissions your application defines by passing them to
`laterite_admin::router`, so they appear in the role editor alongside the
framework's. A `Permission` is a `code`, a human `label`, and the `group` it
sorts under:

```rust
use laterite_admin::Permission;

let permissions = vec![
    Permission {
        code: "acme.publish_pages".to_string(),
        label: "Publish pages".to_string(),
        group: "Content".to_string(),
    },
    Permission {
        code: "acme.manage_media".to_string(),
        label: "Manage media".to_string(),
        group: "Content".to_string(),
    },
];

let app = laterite_admin::router(auth, pool, resources, settings, permissions, config);
```

Gate a screen on one of these by setting it as a resource's `permission` (above),
and it becomes assignable from the role editor.
