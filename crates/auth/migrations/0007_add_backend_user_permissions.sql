-- Per-user permission overrides, keyed by permission code: 1 grants the
-- permission and -1 denies it, taking precedence over the user's roles. A code
-- that is absent inherits the role decision. Roles hold the base grants
-- (backend_roles.permissions); this refines them per user.
alter table backend_users add column permissions jsonb not null default '{}';
