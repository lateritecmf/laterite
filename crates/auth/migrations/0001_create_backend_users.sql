-- Backend users: the operators of the admin surface, kept separate from any
-- application's end-user accounts.
--
-- first_name is required; last_name is optional so mononyms and initials-style
-- names are represented without forcing a fabricated value.
create table backend_users (
    id uuid primary key default gen_random_uuid(),
    username text not null unique,
    email text not null unique,
    first_name text not null,
    last_name text,
    password_hash text not null,
    is_superuser boolean not null default false,
    is_active boolean not null default true,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
