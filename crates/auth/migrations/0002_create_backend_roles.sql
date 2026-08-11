-- Roles carry dotted permission strings; a user's effective permissions are the
-- union across the roles assigned to them.
create table backend_roles (
    id uuid primary key default gen_random_uuid(),
    code text not null unique,
    name text not null,
    permissions text[] not null default '{}',
    created_at timestamptz not null default now()
);
