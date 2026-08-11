-- Sessions are stored by SHA-256 of the opaque bearer token, never the token
-- itself, so a database disclosure does not yield usable session credentials.
create table backend_sessions (
    token_hash text primary key,
    backend_user_id uuid not null references backend_users (id) on delete cascade,
    created_at timestamptz not null default now(),
    last_seen_at timestamptz not null default now(),
    expires_at timestamptz not null
);

create index backend_sessions_user_idx on backend_sessions (backend_user_id);
