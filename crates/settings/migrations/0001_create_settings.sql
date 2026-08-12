-- Settings storage: one JSONB blob per settings model, keyed by a stable code.
-- The generic settings controller reads and writes `value` by `code`; typed
-- settings models serialize their whole struct into `value`.
create table settings (
    code text primary key,
    value jsonb not null default '{}',
    updated_at timestamptz not null default now()
);
