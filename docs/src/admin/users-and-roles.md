# Users and Roles

An operator is a person who signs in to the admin. Accounts are managed under
**Settings → Users → Administrators**, gated by `backend.manage_users`; roles
and what each holds are on [Permissions](permissions.md). Every action on an
account is on the audit log, named after the account it touched.

## Create an operator from the command line

```bash
lat admin create editor --email editor@acme.test --first-name Editor --generate
lat admin reset-password editor --generate
lat admin list
```

`--generate` prints a strong password and marks it temporary; a typed or
prompted password is the operator's own.

## Create an operator from code

```rust
use laterite_auth::NewOperator;
use laterite_core::Actor;

let id = auth
    .create_operator(
        NewOperator {
            username: "editor",
            email: "editor@acme.test",
            first_name: "Editor",
            last_name: None,
            password: &laterite_auth::password::generate(),
            timezone: None,
        },
        &Actor::user(admin.id, &admin.username),
    )
    .await?;
auth.require_password_change(id).await?;

let temporary = auth.reset_operator_password(id, &actor).await?;
auth.unlock("editor").await?;
```

Call | Description
--- | ---
`create_operator(new, actor)` | A new account; the id is returned.
`require_password_change(id)` | Marks the password temporary; the account replaces it at its next sign-in.
`reset_operator_password(id, actor)` | A new temporary password, returned once; every device is signed out.
`unlock(username)` | Clears a sign-in lockout.
`create_superuser` | A superuser, the same way; first-run setup and `lat admin create` use it.

## Invite an operator and reset a password

```text
laterite.auth::invite   name, username, link, expires
laterite.auth::reset    name, username, link, expires
```

Page | Does
--- | ---
`/admin/users/new` | Creates the account, shows the temporary password once, and mails `laterite.auth::invite` with a link to set a password; the link works for seven days.
`/admin/forgot` | Linked from the sign-in page. Takes a username or email address and mails `laterite.auth::reset` with a link that works for one hour; the page says the same whether or not the account exists, and one address may ask five times in fifteen minutes.
`/admin/set-password/<token>` | Opened from either link: a new password, checked against the password policy, then the sign-in page.

Both templates are edited under **Settings → System → Mail Templates**. The mail goes through the configured transport; with the `log` driver it sits in the Mail Log.

