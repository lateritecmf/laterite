//! Bulk import: a file's rows written through a resource's form, with a report.
//!
//! A resource declaring `import: true` gets the pages under its own path: a
//! file taken in, its columns shown beside the form's fields with a match
//! proposed for each, a check that writes nothing, then the run and its report
//! as it fills. Each row is written the way the form's own submit would: the
//! same rules, field types, listeners, persister and audit entry, one commit a
//! row. The run is detached from the request that started it and keeps its
//! counts and notes in a table the report polls, so a long file finishes
//! whether or not the browser waits, and a refused row is a line in the report,
//! never a stopped import. From code, [`start`] and [`find`] do the same
//! without the pages.

pub(crate) mod file;
pub(crate) mod migrations;
mod pages;
pub mod run;
pub mod schema;
mod store;

pub use run::{
    find, start, start_inline, Duplicates, ImportJob, Match, Note, NoteKind, OnMatch, Run,
    RunError, RunSpec, Status,
};

pub(crate) use pages::ImportFeature;

use laterite_core::queue::{Job, JobFailed};
use laterite_core::{EventCx, EventError, Listener, MigrationSet, Module, ModuleId};

/// The name of the disk uploaded files are stored on. The application may
/// register a `laterite_media::DiskReg` under it; without one, the files go to
/// `private` under the configured storage directory.
pub const DISK: &str = "private";

/// The `laterite.import` module: the run table. Built in.
pub struct ImportModule;

impl Module for ImportModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }

    fn migrations(&self) -> MigrationSet {
        migrations::migrations()
    }

    /// A run whose job gave up (a worker lost mid-run, a file that would not
    /// read) is marked failed from the queue's own record of it.
    fn register(&self, registry: &mut laterite_core::Registry) {
        registry.listen::<JobFailed>(RunFailed);
    }
}

struct RunFailed;

#[laterite_core::strata::async_trait]
impl Listener<JobFailed> for RunFailed {
    async fn handle(&self, cx: &EventCx<'_>, event: &JobFailed) -> Result<(), EventError> {
        if event.name != run::ImportJob::NAME {
            return Ok(());
        }
        let Some(run_id) = event.payload.get("run_id").and_then(|v| v.as_i64()) else {
            return Ok(());
        };
        if let Ok(Some(found)) = store::find(cx.db(), run_id).await {
            if matches!(found.status, run::Status::Queued | run::Status::Running) {
                store::fail(cx.db(), run_id, &event.error).await?;
            }
        }
        Ok(())
    }
}
