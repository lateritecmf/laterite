//! The table and column identifiers.

use laterite_core::strata::Iden;

/// The run table: one row per import, from start to report.
#[derive(Iden)]
pub enum ImportRuns {
    Table,
    Id,
    /// The resource whose form the rows went through, as its descriptor
    /// declares the path.
    Resource,
    /// The stored payload's media record, when the file was kept.
    MediaId,
    ActorUserId,
    ActorLabel,
    /// `running`, `done` or `failed`.
    Status,
    /// The [`RunSpec`](crate::RunSpec) as JSON, so a report can say what it ran.
    Spec,
    Processed,
    Created,
    Updated,
    Skipped,
    Errors,
    /// The [`Note`](crate::Note) list as JSON.
    Notes,
    /// Why the run itself stopped, when `status` is `failed`.
    Failure,
    StartedAt,
    FinishedAt,
}
