//! The media schema, as portable one-file migrations.
//!
//! Each migration is one file, listed below in apply order. Append new entries
//! at the end; never reorder or rename a shipped one.

laterite_core::migration_set! {
    module_id: "laterite.media",
    m0001_create_media,
}
