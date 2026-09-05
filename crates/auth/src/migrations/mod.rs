//! The auth schema, as portable one-file migrations.
//!
//! Written with the `sea-query` schema builder so one definition serves
//! Postgres, MySQL, and SQLite. Portability choices: ids are `bigint`
//! auto-increment primary keys (the database assigns them), timestamps are
//! `text` (RFC 3339), and the permission collections are JSON `text` rather than
//! a Postgres array or `jsonb`.
//!
//! Each migration is one file, listed below in apply order. Append new entries
//! at the end; never reorder or rename a shipped one.

laterite_core::migration_set! {
    module_id: "laterite.auth",
    m0001_create_backend_users,
    m0002_create_backend_roles,
    m0003_create_backend_user_roles,
    m0004_create_backend_sessions,
    m0005_create_backend_access_log,
    m0006_add_backend_user_timezone,
    m0007_add_backend_user_permissions,
    m0008_add_backend_session_data,
    m0009_add_backend_user_locale,
    m0010_create_backend_audit_log,
}
