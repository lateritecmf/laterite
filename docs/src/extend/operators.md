# Operators

An operator is a person who signs in to the admin. Accounts are managed under
**Settings → Users → Backend Users**, gated by `backend.manage_users`.

## From the panel

Action | Where | Effect
--- | --- | ---
Create | **New** on the list | Username, email, name, roles. A temporary password is generated and shown once; the account must replace it at first sign-in.
Assign roles | The edit screen | Only roles granting no more than you hold; never your own.
Override a permission | The edit screen | Allow or Deny one permission regardless of roles.
Deactivate | The edit screen | Signs the account out everywhere and refuses sign-in; reversible.

Every action is on the audit log, named after the account it touched.

## From the command line

```bash
lat admin create editor --email editor@acme.test --first-name Editor --generate
lat admin reset-password editor --generate
lat admin list
```

`--generate` prints a strong password and marks it temporary; a typed or
prompted password is the operator's own.

## From code

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
```

`create_superuser` makes a superuser the same way; first-run setup and
`lat admin create` use it.
