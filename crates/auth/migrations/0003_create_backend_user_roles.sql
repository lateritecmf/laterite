-- Assignment of roles to backend users.
create table backend_user_roles (
    backend_user_id uuid not null references backend_users (id) on delete cascade,
    backend_role_id uuid not null references backend_roles (id) on delete cascade,
    primary key (backend_user_id, backend_role_id)
);
