//! Laterite bulk import: a file's rows written through a resource's form, with
//! a report.
//!
//! A run reads a CSV, maps its columns onto a form's fields and writes each row
//! the way the form's own submit would: the same rules, field types, listeners,
//! persister and audit entry, one commit a row. The run is detached from the
//! request that started it and keeps its counts and notes in a table a screen
//! polls, so a long file finishes whether or not the browser waits, and a
//! refused row is a line in the report, never a stopped import.
//!
//! The screen at `/import` takes a file, shows its columns beside the form's
//! fields with a match proposed for each, starts the run and shows the report
//! as it fills. The application says which resources may be imported into with
//! an [`ImportReg`], and which disk keeps the uploaded file with a
//! `laterite_media::DiskReg` named `private`. From code, [`start`] and [`find`]
//! do the same without the screen.

pub mod admin;
mod file;
pub mod migrations;
pub mod run;
pub mod schema;
mod store;

pub use admin::{ImportReg, DISK, PERMISSION};
pub use run::{
    find, start, Duplicates, Match, Note, NoteKind, OnMatch, Run, RunError, RunSpec, Status,
};

use laterite_core::{MigrationSet, Module, ModuleId, Registry};

/// This crate's module. An application that imports registers it on
/// `Bootstrap`; one that does not never compiles this crate at all.
pub fn module() -> Box<dyn Module> {
    Box::new(ImportModule)
}

/// The `laterite.import` module: the run table, the screen and its permission.
pub struct ImportModule;

impl Module for ImportModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }

    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }

    fn register(&self, registry: &mut Registry) {
        admin::contribute(registry);
    }

    /// The screen lives at `/import` under the panel, a path short enough to
    /// type. A module claiming the same path aborts boot, as any clash does.
    fn admin_base(&self) -> Option<&'static str> {
        Some("/import")
    }
}
