//! The settings schema, as a portable one-file migration.
//!
//! Each migration is one file, listed below in apply order. Append new entries
//! at the end; never reorder or rename a shipped one.

laterite_core::migration_set! {
    module_id: "laterite.settings",
    m0001_create_settings,
}
