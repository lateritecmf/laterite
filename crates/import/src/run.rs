//! The run: a file's rows through a form, detached from the request, with a
//! report a screen polls.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use futures::TryStreamExt;
use laterite_admin::form::FormHandle;
use laterite_admin::persist::SaveError;
use laterite_core::query::{bind_values, build, text_cast};
use laterite_core::{t, Actor, AnyRowExt, Db, Text};
use laterite_media::ByteStream;
use sea_query::{Alias, Expr, Query};
use serde::{Deserialize, Serialize};

use crate::store;

/// How one run reads its file and writes its rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RunSpec {
    /// The resource whose form the rows go through, as its descriptor declares
    /// the path (`/products`). Recorded on the run.
    pub resource: String,
    /// The stored payload's media record, kept on the run for the report.
    pub media_id: Option<i64>,
    /// Which file column feeds which field. A column may feed several fields.
    pub matches: Vec<Match>,
    /// What a row does when its key matches a record already there. `None`
    /// creates every row.
    pub duplicates: Option<Duplicates>,
    /// Whether the first row names the columns and is passed over. Default
    /// `true`.
    pub first_row_titles: bool,
    /// The byte between values. Default `,`.
    pub delimiter: u8,
    /// Run every row through the rules and conversions with nothing written,
    /// so the report says what an import would do. Default `false`.
    #[serde(default)]
    pub dry_run: bool,
    /// On an update, a matched cell that is blank leaves the record's value
    /// as it is. Default `true`; `false` clears the field.
    #[serde(default = "yes")]
    pub keep_blank: bool,
    /// The file's column titles, kept so a later file with the same titles
    /// is proposed the same matching.
    #[serde(default)]
    pub titles: Vec<String>,
}

fn yes() -> bool {
    true
}

impl RunSpec {
    /// A run into `resource` with these column matches.
    pub fn new(resource: impl Into<String>, matches: Vec<Match>) -> Self {
        Self {
            resource: resource.into(),
            media_id: None,
            matches,
            duplicates: None,
            first_row_titles: true,
            delimiter: b',',
            dry_run: false,
            keep_blank: true,
            titles: Vec::new(),
        }
    }

    /// Run every row with nothing written; the report says what would happen.
    pub fn dry_run(mut self, dry: bool) -> Self {
        self.dry_run = dry;
        self
    }

    /// Whether a blank matched cell leaves an updated record's value as it is.
    pub fn keep_blank(mut self, keep: bool) -> Self {
        self.keep_blank = keep;
        self
    }

    /// The file's column titles, for a later file to be matched the same way.
    pub fn titles(mut self, titles: Vec<String>) -> Self {
        self.titles = titles;
        self
    }

    /// The stored payload's media record.
    pub fn media(mut self, id: i64) -> Self {
        self.media_id = Some(id);
        self
    }

    /// What a row does when its key matches a record already there.
    pub fn duplicates(mut self, duplicates: Duplicates) -> Self {
        self.duplicates = Some(duplicates);
        self
    }

    /// Whether the first row names the columns.
    pub fn first_row_titles(mut self, titles: bool) -> Self {
        self.first_row_titles = titles;
        self
    }

    /// The byte between values.
    pub fn delimiter(mut self, delimiter: u8) -> Self {
        self.delimiter = delimiter;
        self
    }
}

/// One file column feeding one field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Match {
    /// The column's position in the file, from 0.
    pub column: usize,
    /// The form field it fills.
    pub field: String,
}

impl Match {
    pub fn new(column: usize, field: impl Into<String>) -> Self {
        Self {
            column,
            field: field.into(),
        }
    }
}

/// What a row does when a record already carries its key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Duplicates {
    /// The matched field whose value identifies a record.
    pub key: String,
    pub on_match: OnMatch,
}

impl Duplicates {
    /// A row whose `key` is already there is passed over and noted.
    pub fn skip(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            on_match: OnMatch::Skip,
        }
    }

    /// A row whose `key` is already there updates that record: the file's
    /// columns over the record as it stands.
    pub fn update(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            on_match: OnMatch::Update,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnMatch {
    Skip,
    Update,
}

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Rows are still being written; the counts move.
    Running,
    /// Every row was read; the counts and notes are final.
    Done,
    /// The run itself stopped (the file could not be read, the run table could
    /// not be written); [`Run::failure`] says why. Rows written before it stay.
    Failed,
}

impl Status {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Status::Running),
            "done" => Some(Status::Done),
            "failed" => Some(Status::Failed),
            _ => None,
        }
    }
}

/// One line of the report: what happened to one row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Note {
    /// The row's line in the file, from 1, the title row counted.
    pub row: u64,
    pub kind: NoteKind,
    /// The field the message is about, when it is about one.
    pub field: Option<String>,
    /// Localized where the report renders, like a validation message.
    pub message: Text,
}

impl Note {
    fn new(kind: NoteKind, row: u64, field: Option<String>, message: Text) -> Self {
        Self {
            row,
            kind,
            field,
            message,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    /// The row was not written.
    Error,
    /// The row was passed over by the duplicates policy.
    Skipped,
}

/// A run as the table holds it: what it ran, where it is, what it did.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Run {
    pub id: i64,
    pub resource: String,
    pub media_id: Option<i64>,
    /// What it ran: the matches, the policy, and whether anything was written.
    pub spec: RunSpec,
    pub actor_label: String,
    pub status: Status,
    /// Rows read, blank rows left out.
    pub processed: i64,
    pub created: i64,
    pub updated: i64,
    pub skipped: i64,
    /// Rows refused.
    pub errors: i64,
    pub notes: Vec<Note>,
    pub failure: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RunError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    /// The spec does not fit the form: a match naming a field it lacks, no
    /// matches at all, a duplicates key no column feeds.
    #[error("the run cannot start: {0}")]
    Spec(String),
    #[error("the file could not be read: {0}")]
    Read(#[from] std::io::Error),
}

/// Rows counted as the run goes.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Counts {
    pub processed: i64,
    pub created: i64,
    pub updated: i64,
    pub skipped: i64,
    pub errors: i64,
}

/// How many rows between writes of the counts, so a poll sees movement.
const PROGRESS_EVERY: i64 = 25;

/// The runs this process is working on. A row still `running` whose id is not
/// here was left by a process that stopped mid-run; [`find`] reports it failed.
static LIVE: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// How old a `running` row has to be before its absence from [`LIVE`] means it
/// was lost, so a run between its insert and its start is never mistaken.
const STALE_AFTER_SECONDS: i64 = 5;

fn live(id: i64, is: bool) {
    let mut set = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if is {
        set.insert(id);
    } else {
        set.remove(&id);
    }
}

fn is_live(id: i64) -> bool {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).contains(&id)
}

/// Starts a run: records it, detaches the work and returns the run's id for
/// [`find`] to poll.
///
/// `source` is the file's bytes; `actor` is who the rows are written as, on
/// every record and audit entry. The spec is checked against the form first,
/// so a mistaken screen fails here and no run row is left behind.
pub async fn start(
    db: &Db,
    form: FormHandle,
    actor: Actor,
    spec: RunSpec,
    source: ByteStream,
) -> Result<i64, RunError> {
    check(&spec, &form)?;
    let id = store::insert(db, &spec, &actor).await?;
    live(id, true);
    let db = db.clone();
    tokio::spawn(async move {
        if let Err(e) = run(&db, id, &form, &actor, &spec, source).await {
            tracing::error!(run = id, error = %e, "import run stopped");
            if let Err(e) = store::fail(&db, id, &e.to_string()).await {
                tracing::error!(run = id, error = %e, "import run could not be marked failed");
            }
        }
        live(id, false);
    });
    Ok(id)
}

/// One run by id, or `None` when there is no such run.
///
/// A run still recorded as running that this process is not working on was
/// lost when a process stopped mid-run; it is marked failed here, with its
/// counts as they were, so a report never shows a run that will not move.
pub async fn find(db: &Db, id: i64) -> Result<Option<Run>, RunError> {
    let Some(run) = store::find(db, id).await? else {
        return Ok(None);
    };
    if run.status == Status::Running && !is_live(run.id) && is_stale(&run.started_at) {
        store::fail(db, run.id, "The server stopped while the run was going.").await?;
        return Ok(store::find(db, run.id).await?);
    }
    Ok(Some(run))
}

fn is_stale(started_at: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|at| {
            (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds()
                > STALE_AFTER_SECONDS
        })
        .unwrap_or(true)
}

fn check(spec: &RunSpec, form: &FormHandle) -> Result<(), RunError> {
    if spec.matches.is_empty() {
        return Err(RunError::Spec(
            "no column is matched to a field".to_string(),
        ));
    }
    let known = |name: &str| form.fields().iter().any(|f| f.name == name);
    for m in &spec.matches {
        if !known(&m.field) {
            return Err(RunError::Spec(format!(
                "column {} is matched to `{}`, which is not a field of this form",
                m.column, m.field
            )));
        }
    }
    if let Some(duplicates) = &spec.duplicates {
        if !spec.matches.iter().any(|m| m.field == duplicates.key) {
            return Err(RunError::Spec(format!(
                "duplicates are keyed on `{}`, which no column feeds",
                duplicates.key
            )));
        }
    }
    Ok(())
}

async fn run(
    db: &Db,
    id: i64,
    form: &FormHandle,
    actor: &Actor,
    spec: &RunSpec,
    source: ByteStream,
) -> Result<(), RunError> {
    let bytes: Vec<u8> = source
        .try_fold(Vec::new(), |mut all, chunk| async move {
            all.extend_from_slice(&chunk);
            Ok(all)
        })
        .await?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(spec.delimiter)
        .has_headers(spec.first_row_titles)
        .flexible(true)
        .from_reader(bytes.as_slice());

    let mut counts = Counts::default();
    let mut notes = Vec::new();
    for record in reader.records() {
        let record = match record {
            Ok(record) => record,
            Err(e) => {
                let row = e.position().map(|p| p.line()).unwrap_or(0);
                counts.processed += 1;
                counts.errors += 1;
                notes.push(Note::new(
                    NoteKind::Error,
                    row,
                    None,
                    t!(
                        "The row could not be read: {reason}",
                        reason = e.to_string()
                    ),
                ));
                continue;
            }
        };
        // A blank line is nothing to import, so it is neither a row nor a note.
        if record.iter().all(|v| v.trim().is_empty()) {
            continue;
        }
        let row = record.position().map(|p| p.line()).unwrap_or(0);
        let mut data = HashMap::new();
        for m in &spec.matches {
            data.insert(
                m.field.clone(),
                record.get(m.column).unwrap_or_default().to_string(),
            );
        }
        counts.processed += 1;
        write_row(db, form, actor, spec, row, data, &mut counts, &mut notes).await;
        if counts.processed % PROGRESS_EVERY == 0 {
            store::progress(db, id, &counts).await?;
        }
    }
    store::finish(db, id, &counts, &notes).await?;
    Ok(())
}

/// Writes one row as its spec says, counting what happened and noting anything
/// the operator should hear about.
#[allow(clippy::too_many_arguments)]
async fn write_row(
    db: &Db,
    form: &FormHandle,
    actor: &Actor,
    spec: &RunSpec,
    row: u64,
    data: HashMap<String, String>,
    counts: &mut Counts,
    notes: &mut Vec<Note>,
) {
    if let Some(duplicates) = &spec.duplicates {
        let key = data.get(&duplicates.key).cloned().unwrap_or_default();
        if !key.is_empty() {
            let found = match existing(db, form, &duplicates.key, &key).await {
                Ok(found) => found,
                Err(e) => {
                    tracing::error!(row, error = %e, "import duplicate lookup failed");
                    counts.errors += 1;
                    notes.push(Note::new(
                        NoteKind::Error,
                        row,
                        Some(duplicates.key.clone()),
                        t!("The existing record could not be looked up."),
                    ));
                    return;
                }
            };
            if let Some(id) = found {
                match duplicates.on_match {
                    OnMatch::Skip => {
                        counts.skipped += 1;
                        notes.push(Note::new(
                            NoteKind::Skipped,
                            row,
                            Some(duplicates.key.clone()),
                            t!(
                                "A record with this {field} is already there.",
                                field = duplicates.key.clone()
                            ),
                        ));
                    }
                    OnMatch::Update => {
                        // The file's columns over the record as it stands, so a
                        // field the file does not carry keeps its value, and so
                        // does one whose cell is blank when the spec says so.
                        let current = match form.load(id).await {
                            Ok(current) => current.unwrap_or_default(),
                            Err(e) => {
                                refused(row, e, counts, notes);
                                return;
                            }
                        };
                        let mut merged = current;
                        for (field, value) in data {
                            if spec.keep_blank && value.trim().is_empty() {
                                continue;
                            }
                            merged.insert(field, value);
                        }
                        let outcome = if spec.dry_run {
                            form.check(Some(id), merged).await
                        } else {
                            form.update(id, merged, actor).await
                        };
                        match outcome {
                            Ok(()) => counts.updated += 1,
                            Err(e) => refused(row, e, counts, notes),
                        }
                    }
                }
                return;
            }
        }
    }
    let outcome = if spec.dry_run {
        form.check(None, data).await
    } else {
        form.create(data, actor).await.map(|_| ())
    };
    match outcome {
        Ok(()) => counts.created += 1,
        Err(e) => refused(row, e, counts, notes),
    }
}

/// Notes a row the form would not take: one line per message, as the form
/// would show them, or the reason a write failed.
fn refused(row: u64, error: SaveError, counts: &mut Counts, notes: &mut Vec<Note>) {
    counts.errors += 1;
    match error {
        SaveError::Invalid(bag) => {
            for (field, messages) in bag.iter() {
                for message in messages {
                    notes.push(Note::new(
                        NoteKind::Error,
                        row,
                        Some(field.to_string()),
                        message.clone(),
                    ));
                }
            }
        }
        SaveError::Failed(reason) => {
            tracing::warn!(row, error = %reason, "import row could not be saved");
            notes.push(Note::new(
                NoteKind::Error,
                row,
                None,
                t!("Could not save: {reason}", reason = reason),
            ));
        }
    }
}

/// The id of the record whose `key` column holds `value`, if one does.
async fn existing(
    db: &Db,
    form: &FormHandle,
    key: &str,
    value: &str,
) -> Result<Option<i64>, sqlx::Error> {
    // Scoped so the builder is dropped before the await: its identifiers are
    // reference-counted, and a live one across the await would make the run's
    // future non-`Send`.
    let (sql, values) = {
        let cast = text_cast(db.backend);
        build(
            db.backend,
            Query::select()
                .column(Alias::new(form.id_field()))
                .from(Alias::new(form.entity()))
                .and_where(
                    Expr::col(Alias::new(key))
                        .cast_as(Alias::new(cast))
                        .eq(value),
                )
                .limit(1)
                .to_owned(),
        )
    };
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.map(|r| r.get_int(form.id_field())).transpose()
}
