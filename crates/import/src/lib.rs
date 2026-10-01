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
//! This crate currently carries the engine: the run table, `start` and `find`.
//! The upload and column-matching screen and the report page arrive next.

pub mod migrations;
pub mod run;
pub mod schema;
mod store;

pub use run::{
    find, start, Duplicates, Match, Note, NoteKind, OnMatch, Run, RunError, RunSpec, Status,
};

use laterite_core::{MigrationSet, Module, ModuleId};

/// This crate's module. An application that imports registers it on
/// `Bootstrap`; one that does not never compiles this crate at all.
pub fn module() -> Box<dyn Module> {
    Box::new(ImportModule)
}

/// The `laterite.import` module: the run table.
pub struct ImportModule;

impl Module for ImportModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }

    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }
}
