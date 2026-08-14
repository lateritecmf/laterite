-- Per-operator display timezone. Null means the operator inherits the
-- deployment default (backend.timezone). Storage is always UTC; this only
-- changes how timestamps render in the admin for this operator.
alter table backend_users add column timezone text;
