-- Append-only record of every authentication outcome, for auditing and for the
-- failed-attempt throttle.
create table backend_access_log (
    id bigint generated always as identity primary key,
    backend_user_id uuid references backend_users (id) on delete set null,
    username_attempted text not null,
    event text not null,
    ip_address text,
    user_agent text,
    created_at timestamptz not null default now()
);

create index backend_access_log_username_idx
    on backend_access_log (username_attempted, created_at);
