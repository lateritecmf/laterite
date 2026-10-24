//! The mail schema, as a portable one-file migration.
//!
//! Each migration is one file, listed below in apply order. Append new entries
//! at the end; never reorder or rename a shipped one.

crate::migration_set! {
    module_id: "laterite.mail",
    m0001_create_mail,
}
