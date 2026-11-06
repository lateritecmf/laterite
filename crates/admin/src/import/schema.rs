//! The table and column identifiers.

use laterite_core::strata::Iden;

/// The run table: one row per import, from start to report.
#[derive(Iden)]
pub enum ImportRuns {
    /// The table itself, `import_runs`.
    Table,
    /// The run's id, bigint auto-increment.
    Id,
    /// The resource whose form the rows went through, as its descriptor
    /// declares the path.
    Resource,
    /// The stored payload's media record, when the file was kept.
    MediaId,
    /// Id of the user who started the run.
    ActorUserId,
    /// Display label of the user who started the run.
    ActorLabel,
    /// `running`, `done` or `failed`.
    Status,
    /// The [`RunSpec`](crate::import::RunSpec) as JSON, so a report can say what it ran.
    Spec,
    /// Rows read, blank rows left out.
    Processed,
    /// Rows written as new records.
    Created,
    /// Rows written over an existing record.
    Updated,
    /// Rows passed over by the duplicates policy.
    Skipped,
    /// Rows refused.
    Errors,
    /// The [`Note`](crate::import::Note) list as JSON.
    Notes,
    /// Why the run itself stopped, when `status` is `failed`.
    Failure,
    /// UTC timestamp, as text, of when the run was queued and then started.
    StartedAt,
    /// UTC timestamp, as text, of when the run finished.
    FinishedAt,
}
