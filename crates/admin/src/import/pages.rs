//! The pages under a resource: a file taken in, its columns matched, a check
//! or a run started, the report watched, the refused rows handed back.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::html::Markup;
use crate::routes::{ResourceFeature, ResourceInfo, RouteCtx};
use crate::{AdminError, FlashLevel, SessionHandle, Shell, VerifiedUpload};
use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use futures::TryStreamExt;
use laterite_auth::AuthenticatedUser;
use laterite_core::validation::Rule;
use laterite_core::{t, Actor, Db, Text};
use laterite_media::{blob_path, ingest, record, stream_of, NewMedia, StorageDriver};
use serde::Deserialize;

use super::DISK;
use crate::import::file::{peek, sniff_delimiter, PEEK_BYTES};
use crate::import::run::{
    find, start, Duplicates, Match, Note, NoteKind, Run, RunError, RunSpec, Status,
};
use crate::import::store;

/// The largest file the pages take in.
const UPLOAD_LIMIT: u64 = 20 * 1024 * 1024;

/// How many runs the index lists.
const RECENT_RUNS: i64 = 20;

/// The pages the router mounts under a resource declaring `import`.
pub(crate) struct ImportFeature {
    /// Where uploaded files are kept: the application's `private` disk, else the
    /// default under the storage directory.
    pub(crate) disk: Arc<dyn StorageDriver>,
}

/// What the handlers need after mount, cloned per request.
#[derive(Clone)]
struct App {
    db: Db,
    queue: laterite_core::Queue,
    base: Arc<str>,
    disk: Arc<dyn StorageDriver>,
    info: Arc<ResourceInfo>,
}

impl App {
    fn url(&self, relative: &str) -> String {
        let base = self.base.trim_end_matches('/');
        let relative = relative.trim_start_matches('/');
        if relative.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{relative}")
        }
    }

    fn label(&self, shell: &Shell) -> String {
        shell.tt(&self.info.label)
    }
}

impl ResourceFeature for ImportFeature {
    fn mount(&self, ctx: &RouteCtx, resource: &ResourceInfo) -> Router {
        let app = App {
            db: ctx.db().clone(),
            queue: ctx.queue().clone(),
            base: Arc::from(ctx.base_path()),
            disk: self.disk.clone(),
            info: Arc::new(resource.clone()),
        };
        Router::new()
            .route("/", get(index).post(upload))
            .route("/template.csv", get(template))
            .route("/{media}", get(match_form).post(start_run))
            .route("/runs/{id}", get(run_page))
            .route("/runs/{id}/status", get(run_status))
            .route("/runs/{id}/import", post(import_now))
            .route("/runs/{id}/refused.csv", get(refused_csv))
            .with_state(app)
    }
}

// The index: the upload form and this resource's runs.

#[derive(Template)]
#[template(path = "import/index.html")]
struct IndexTemplate<'a> {
    shell: &'a Shell,
    action: String,
    template: String,
    back: String,
    back_label: String,
    runs: Vec<RunRow>,
}

struct RunRow {
    id: i64,
    href: String,
    kind: String,
    status: String,
    status_class: &'static str,
    created: i64,
    updated: i64,
    skipped: i64,
    errors: i64,
    started_at: String,
    actor: String,
}

fn run_row(app: &App, shell: &Shell, run: &Run) -> RunRow {
    RunRow {
        id: run.id,
        href: app.url(&format!("runs/{}", run.id)),
        kind: kind_label(shell, run),
        status: status_label(shell, run.status),
        status_class: status_class(run.status),
        created: run.created,
        updated: run.updated,
        skipped: run.skipped,
        errors: run.errors,
        started_at: shell.datetime(&run.started_at),
        actor: run.actor_label.clone(),
    }
}

async fn index(State(app): State<App>, Extension(shell): Extension<Shell>) -> Response {
    let runs = match store::recent_for(&app.db, &app.info.path, RECENT_RUNS).await {
        Ok(runs) => runs,
        Err(e) => return AdminError::Internal(e.into()).into_response(),
    };
    let runs = runs.iter().map(|run| run_row(&app, &shell, run)).collect();
    page(
        &shell,
        &t!("Import into {resource}", resource = app.label(&shell)),
        &IndexTemplate {
            shell: &shell,
            action: app.url(""),
            template: app.url("template.csv"),
            back: app.info.base_path.clone(),
            back_label: shell.tt(&t!("Back to {resource}", resource = app.label(&shell))),
            runs,
        },
    )
}

/// A file with the form's fields as its column titles, to fill in.
async fn template(State(app): State<App>) -> Response {
    let names: Vec<&str> = app
        .info
        .form
        .fields()
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let body = format!("\u{feff}{}\r\n", names.join(","));
    csv_download(
        &format!(
            "{}-template.csv",
            app.info.path.trim_start_matches('/').replace('/', "-")
        ),
        body,
    )
}

fn csv_download(name: &str, body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        body,
    )
        .into_response()
}

/// How the file is read: carried from the upload to the matching step and on
/// to the run.
#[derive(Debug, Clone, Deserialize)]
struct Reading {
    #[serde(default = "titles_on")]
    titles: String,
    #[serde(default = "auto")]
    delimiter: String,
}

fn titles_on() -> String {
    "1".to_string()
}

fn auto() -> String {
    "auto".to_string()
}

impl Reading {
    fn first_row_titles(&self) -> bool {
        self.titles == "1"
    }

    /// The delimiter chosen, or the one the file's first line suggests.
    fn delimiter_byte(&self, head: &[u8]) -> u8 {
        match self.delimiter.as_str() {
            "," => b',',
            ";" => b';',
            "tab" => b'\t',
            _ => sniff_delimiter(head),
        }
    }

    fn query(&self) -> String {
        format!(
            "?titles={}&delimiter={}",
            if self.first_row_titles() { "1" } else { "0" },
            match self.delimiter.as_str() {
                "," => "%2C",
                ";" => "%3B",
                "tab" => "tab",
                _ => "auto",
            }
        )
    }
}

async fn upload(
    State(app): State<App>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    mut upload: VerifiedUpload,
) -> Response {
    let back = app.url("");
    let mut reading = Reading {
        titles: "0".to_string(),
        delimiter: auto(),
    };
    let mut file: Option<(String, Vec<u8>)> = None;
    while let Ok(Some(mut field)) = upload.next_field().await {
        match field.name().unwrap_or_default() {
            "titles" => reading.titles = field.text().await.unwrap_or_default(),
            "delimiter" => reading.delimiter = field.text().await.unwrap_or_default(),
            "file" => {
                let name = field.file_name().unwrap_or("import.csv").to_string();
                let mut bytes = Vec::new();
                let mut too_large = false;
                while let Ok(Some(chunk)) = field.chunk().await {
                    if bytes.len() as u64 + chunk.len() as u64 > UPLOAD_LIMIT {
                        too_large = true;
                        break;
                    }
                    bytes.extend_from_slice(&chunk);
                }
                if too_large {
                    session.push_flash(
                        FlashLevel::Error,
                        t!(
                            "The file is larger than {limit} MB.",
                            limit = (UPLOAD_LIMIT / (1024 * 1024)) as i64
                        ),
                    );
                    return Redirect::to(&back).into_response();
                }
                file = Some((name, bytes));
            }
            _ => {}
        }
    }
    let Some((name, bytes)) = file.filter(|(_, b)| !b.is_empty()) else {
        session.push_flash(FlashLevel::Error, t!("Choose a file to import."));
        return Redirect::to(&back).into_response();
    };
    let stored = match ingest(app.disk.as_ref(), stream_of(bytes), UPLOAD_LIMIT).await {
        Ok(stored) => stored,
        Err(e) => {
            tracing::error!(error = %e, "import upload could not be stored");
            session.push_flash(FlashLevel::Error, t!("The file could not be stored."));
            return Redirect::to(&back).into_response();
        }
    };
    let media = record::create(
        &app.db,
        NewMedia {
            disk: DISK.to_string(),
            hash: stored.hash,
            size: stored.size as i64,
            content_type: stored.content_type,
            original_name: name,
            in_library: false,
            created_by: Some(user.user.id),
        },
    )
    .await;
    match media {
        Ok(id) => Redirect::to(&format!("{}{}", app.url(&id.to_string()), reading.query()))
            .into_response(),
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}

// The matching step.

#[derive(Template)]
#[template(path = "import/match.html")]
struct MatchTemplate<'a> {
    shell: &'a Shell,
    action: String,
    back: String,
    file_name: String,
    titles: bool,
    delimiter: String,
    detected: String,
    columns: Vec<ColumnRow>,
    fields: Vec<FieldRow>,
    key: String,
    on_match: String,
    keep_blank: bool,
    remembered: bool,
    error: Option<String>,
}

struct ColumnRow {
    index: usize,
    title: String,
    sample: String,
    selected: String,
}

struct FieldRow {
    name: String,
    label: String,
    required: bool,
}

/// The stored file behind `media`, when it is one `user` uploaded here.
async fn payload(
    app: &App,
    media: i64,
    user: &AuthenticatedUser,
) -> Result<record::MediaRecord, AdminError> {
    let record = record::find(&app.db, media)
        .await
        .map_err(|e| AdminError::Internal(e.into()))?
        .ok_or(AdminError::NotFound)?;
    let own = record.created_by == Some(user.user.id) || user.permissions.is_superuser();
    if record.disk != DISK || record.in_library || !own {
        return Err(AdminError::NotFound);
    }
    Ok(record)
}

/// The first bytes of a stored file.
async fn head_of(app: &App, record: &record::MediaRecord) -> Result<Vec<u8>, AdminError> {
    let mut stream = app
        .disk
        .get(&blob_path(&record.hash))
        .await
        .map_err(|e| AdminError::Internal(e.into()))?;
    let mut bytes = Vec::new();
    while bytes.len() < PEEK_BYTES {
        match stream.try_next().await {
            Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(e) => return Err(AdminError::Internal(e.into())),
        }
    }
    bytes.truncate(PEEK_BYTES);
    Ok(bytes)
}

/// A title and a field name folded for comparison: letters and digits, lowercase.
fn folded(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn field_rows(app: &App, shell: &Shell) -> Vec<FieldRow> {
    app.info
        .form
        .fields()
        .iter()
        .map(|f| FieldRow {
            name: f.name.clone(),
            label: shell.tt(&f.label),
            required: f.rules.iter().any(|r| matches!(r, Rule::Required)),
        })
        .collect()
}

/// What the matching step proposes: the operator's own choices on a refused
/// submit, else the last run over a file with these titles, else the field
/// whose name or label reads like each title.
struct Proposal {
    matches: HashMap<usize, String>,
    key: String,
    on_match: String,
    keep_blank: bool,
    remembered: bool,
}

async fn propose(
    app: &App,
    titles: &[String],
    fields: &[FieldRow],
    chosen: &HashMap<String, String>,
) -> Proposal {
    if !chosen.is_empty() {
        let matches = chosen
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("match_")
                    .and_then(|i| i.parse().ok())
                    .map(|i| (i, v.clone()))
            })
            .collect();
        return Proposal {
            matches,
            key: chosen.get("key").cloned().unwrap_or_default(),
            on_match: chosen
                .get("on_match")
                .cloned()
                .unwrap_or_else(|| "skip".to_string()),
            keep_blank: chosen.contains_key("keep_blank"),
            remembered: false,
        };
    }
    if !titles.is_empty() {
        if let Ok(Some(last)) = store::last_for(&app.db, &app.info.path).await {
            if last.spec.titles == titles {
                return Proposal {
                    matches: last
                        .spec
                        .matches
                        .iter()
                        .map(|m| (m.column, m.field.clone()))
                        .collect(),
                    key: last
                        .spec
                        .duplicates
                        .as_ref()
                        .map(|d| d.key.clone())
                        .unwrap_or_default(),
                    on_match: match last.spec.duplicates.as_ref().map(|d| d.on_match) {
                        Some(crate::import::run::OnMatch::Update) => "update".to_string(),
                        _ => "skip".to_string(),
                    },
                    keep_blank: last.spec.keep_blank,
                    remembered: true,
                };
            }
        }
    }
    let matches = titles
        .iter()
        .enumerate()
        .filter_map(|(index, title)| {
            let want = folded(title);
            fields
                .iter()
                .find(|f| folded(&f.name) == want || folded(&f.label) == want)
                .map(|f| (index, f.name.clone()))
        })
        .collect();
    Proposal {
        matches,
        key: String::new(),
        on_match: "skip".to_string(),
        keep_blank: true,
        remembered: false,
    }
}

async fn render_match(
    app: &App,
    shell: &Shell,
    record: &record::MediaRecord,
    reading: &Reading,
    chosen: &HashMap<String, String>,
    error: Option<String>,
) -> Response {
    let bytes = match head_of(app, record).await {
        Ok(bytes) => bytes,
        Err(e) => return e.into_response(),
    };
    let delimiter = reading.delimiter_byte(&bytes);
    let look = peek(&bytes, delimiter, reading.first_row_titles());
    let fields = field_rows(app, shell);
    let proposal = propose(app, &look.titles, &fields, chosen).await;
    let columns = look
        .titles
        .iter()
        .enumerate()
        .map(|(index, title)| ColumnRow {
            index,
            title: title.clone(),
            sample: look
                .samples
                .iter()
                .filter_map(|row| row.get(index))
                .filter(|v| !v.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            selected: proposal.matches.get(&index).cloned().unwrap_or_default(),
        })
        .collect();
    page(
        shell,
        &t!("Import into {resource}", resource = app.label(shell)),
        &MatchTemplate {
            shell,
            action: app.url(&record.id.to_string()),
            back: app.url(""),
            file_name: record.original_name.clone(),
            titles: reading.first_row_titles(),
            delimiter: reading.delimiter.clone(),
            detected: match delimiter {
                b';' => shell.t("Semicolon"),
                b'\t' => shell.t("Tab"),
                _ => shell.t("Comma"),
            },
            columns,
            fields,
            key: proposal.key,
            on_match: proposal.on_match,
            keep_blank: proposal.keep_blank,
            remembered: proposal.remembered,
            error,
        },
    )
}

async fn match_form(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(media): Path<i64>,
    Query(reading): Query<Reading>,
) -> Response {
    let record = match payload(&app, media, &user).await {
        Ok(record) => record,
        Err(e) => return e.into_response(),
    };
    render_match(&app, &shell, &record, &reading, &HashMap::new(), None).await
}

async fn start_run(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(media): Path<i64>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    let record = match payload(&app, media, &user).await {
        Ok(record) => record,
        Err(e) => return e.into_response(),
    };
    let reading = Reading {
        titles: data.get("titles").cloned().unwrap_or_else(titles_on),
        delimiter: data.get("delimiter").cloned().unwrap_or_else(auto),
    };
    let refused = |error: Text| {
        let message = shell.tt(&error);
        render_match(&app, &shell, &record, &reading, &data, Some(message))
    };

    let mut matches = Vec::new();
    for (k, field) in &data {
        if let Some(index) = k.strip_prefix("match_") {
            if let (Ok(column), false) = (index.parse::<usize>(), field.is_empty()) {
                matches.push(Match::new(column, field.clone()));
            }
        }
    }
    matches.sort_by_key(|m| m.column);
    if matches.is_empty() {
        return refused(t!("Match at least one column to a field.")).await;
    }
    let fields = field_rows(&app, &shell);
    if let Some(missing) = fields
        .iter()
        .find(|f| f.required && !matches.iter().any(|m| m.field == f.name))
    {
        return refused(t!(
            "{field} must be matched to a column.",
            field = missing.label.clone()
        ))
        .await;
    }
    let head = match head_of(&app, &record).await {
        Ok(head) => head,
        Err(e) => return e.into_response(),
    };
    let delimiter = reading.delimiter_byte(&head);
    let titles = peek(&head, delimiter, reading.first_row_titles()).titles;
    let mut spec = RunSpec::new(&app.info.path, matches)
        .media(record.id)
        .first_row_titles(reading.first_row_titles())
        .delimiter(delimiter)
        .dry_run(data.get("action").map(String::as_str) != Some("import"))
        .keep_blank(data.contains_key("keep_blank"))
        .titles(titles);
    if let Some(key) = data.get("key").filter(|k| !k.is_empty()) {
        if !spec.matches.iter().any(|m| &m.field == key) {
            return refused(t!(
                "The field records are found by must itself be matched to a column."
            ))
            .await;
        }
        spec = spec.duplicates(match data.get("on_match").map(String::as_str) {
            Some("update") => Duplicates::update(key),
            _ => Duplicates::skip(key),
        });
    }
    launch(&app, &shell, &record, spec, &user).await
}

/// Starts `spec` over the stored file and sends the operator to its report.
async fn launch(
    app: &App,
    shell: &Shell,
    record: &record::MediaRecord,
    spec: RunSpec,
    user: &AuthenticatedUser,
) -> Response {
    match start(
        &app.db,
        &app.queue,
        &app.info.path,
        &app.info.form,
        Actor::from(user),
        spec,
        &record.hash,
    )
    .await
    {
        Ok(id) => Redirect::to(&app.url(&format!("runs/{id}"))).into_response(),
        Err(RunError::Spec(reason)) => {
            let _ = shell;
            AdminError::Internal(anyhow::anyhow!(reason)).into_response()
        }
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}

/// Runs a checked file for real, with the matching the check used.
async fn import_now(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Response {
    let run = match load_run(&app, id).await {
        Ok(run) => run,
        Err(e) => return e.into_response(),
    };
    let Some(media) = run.media_id else {
        return AdminError::NotFound.into_response();
    };
    let record = match payload(&app, media, &user).await {
        Ok(record) => record,
        Err(e) => return e.into_response(),
    };
    let spec = run.spec.clone().dry_run(false);
    launch(&app, &shell, &record, spec, &user).await
}

// The report.

#[derive(Template)]
#[template(path = "import/run.html")]
struct RunTemplate<'a> {
    shell: &'a Shell,
    kind: String,
    file_name: String,
    actor: String,
    started_at: String,
    status: Markup,
    back: String,
}

#[derive(Template)]
#[template(path = "import/_run_status.html")]
struct StatusTemplate<'a> {
    shell: &'a Shell,
    poll: Option<String>,
    dry: bool,
    status: String,
    status_class: &'static str,
    run: &'a Run,
    failure: Option<String>,
    finished_at: Option<String>,
    notes: Vec<NoteRow>,
    /// The real run a finished check offers, while the file is still there.
    import_action: Option<String>,
    /// The refused rows as a file, once there are any.
    refused_href: Option<String>,
}

struct NoteRow {
    row: u64,
    kind: String,
    field: String,
    message: String,
}

fn kind_label(shell: &Shell, run: &Run) -> String {
    if run.spec.dry_run {
        shell.t("Check")
    } else {
        shell.t("Import")
    }
}

fn status_label(shell: &Shell, status: Status) -> String {
    match status {
        Status::Queued => shell.t("Queued, waiting for a worker"),
        Status::Running => shell.t("Running"),
        Status::Done => shell.t("Done"),
        Status::Failed => shell.t("Failed"),
    }
}

fn status_class(status: Status) -> &'static str {
    match status {
        Status::Queued => "lat-status--off",
        Status::Running => "lat-status--running",
        Status::Done => "lat-status--on",
        Status::Failed => "lat-status--off",
    }
}

fn status_markup(app: &App, shell: &Shell, run: &Run) -> Result<Markup, askama::Error> {
    let notes = run
        .notes
        .iter()
        .map(|n: &Note| NoteRow {
            row: n.row,
            kind: match n.kind {
                NoteKind::Error => shell.t("Refused"),
                NoteKind::Skipped => shell.t("Skipped"),
            },
            field: n.field.clone().unwrap_or_default(),
            message: shell.tt(&n.message),
        })
        .collect();
    let settled = matches!(run.status, Status::Done | Status::Failed);
    Markup::from_template(&StatusTemplate {
        shell,
        poll: (!settled).then(|| app.url(&format!("runs/{}/status", run.id))),
        dry: run.spec.dry_run,
        status: status_label(shell, run.status),
        status_class: status_class(run.status),
        run,
        failure: run.failure.clone(),
        finished_at: run.finished_at.as_deref().map(|at| shell.datetime(at)),
        notes,
        import_action: (settled
            && run.spec.dry_run
            && run.status == Status::Done
            && run.media_id.is_some())
        .then(|| app.url(&format!("runs/{}/import", run.id))),
        refused_href: (settled && run.errors > 0 && run.media_id.is_some())
            .then(|| app.url(&format!("runs/{}/refused.csv", run.id))),
    })
}

async fn load_run(app: &App, id: i64) -> Result<Run, AdminError> {
    let run = find(&app.db, id)
        .await
        .map_err(|e| AdminError::Internal(e.into()))?
        .ok_or(AdminError::NotFound)?;
    // Another resource's run is not this page's business.
    if run.resource != app.info.path {
        return Err(AdminError::NotFound);
    }
    Ok(run)
}

async fn run_page(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Path(id): Path<i64>,
) -> Response {
    let run = match load_run(&app, id).await {
        Ok(run) => run,
        Err(e) => return e.into_response(),
    };
    let status = match status_markup(&app, &shell, &run) {
        Ok(status) => status,
        Err(e) => return AdminError::Internal(e.into()).into_response(),
    };
    let file_name = match run.media_id {
        Some(media) => record::find(&app.db, media)
            .await
            .ok()
            .flatten()
            .map(|r| r.original_name)
            .unwrap_or_default(),
        None => String::new(),
    };
    page(
        &shell,
        &t!("Import run {id}", id = run.id),
        &RunTemplate {
            shell: &shell,
            kind: kind_label(&shell, &run),
            file_name,
            actor: run.actor_label.clone(),
            started_at: shell.datetime(&run.started_at),
            status,
            back: app.url(""),
        },
    )
}

async fn run_status(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Path(id): Path<i64>,
) -> Response {
    let run = match load_run(&app, id).await {
        Ok(run) => run,
        Err(e) => return e.into_response(),
    };
    match status_markup(&app, &shell, &run) {
        Ok(status) => Html(status.into_string()).into_response(),
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}

/// The rows the run refused, as they were in the file, with a last column
/// saying why, so they can be fixed and imported on their own.
async fn refused_csv(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<i64>,
) -> Response {
    let run = match load_run(&app, id).await {
        Ok(run) => run,
        Err(e) => return e.into_response(),
    };
    let Some(media) = run.media_id else {
        return AdminError::NotFound.into_response();
    };
    let record = match payload(&app, media, &user).await {
        Ok(record) => record,
        Err(e) => return e.into_response(),
    };
    let bytes: Vec<u8> = match app.disk.get(&blob_path(&record.hash)).await {
        Ok(stream) => match stream
            .try_fold(Vec::new(), |mut all, chunk| async move {
                all.extend_from_slice(&chunk);
                Ok(all)
            })
            .await
        {
            Ok(bytes) => bytes,
            Err(e) => return AdminError::Internal(e.into()).into_response(),
        },
        Err(e) => return AdminError::Internal(e.into()).into_response(),
    };
    let mut problems: HashMap<u64, Vec<String>> = HashMap::new();
    for note in run.notes.iter().filter(|n| n.kind == NoteKind::Error) {
        let message = match &note.field {
            Some(field) => format!("{field}: {}", shell.tt(&note.message)),
            None => shell.tt(&note.message),
        };
        problems.entry(note.row).or_default().push(message);
    }
    let wanted: BTreeSet<u64> = problems.keys().copied().collect();
    let mut writer = csv::WriterBuilder::new()
        .delimiter(run.spec.delimiter)
        .from_writer(Vec::new());
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(run.spec.delimiter)
        .has_headers(run.spec.first_row_titles)
        .flexible(true)
        .from_reader(bytes.as_slice());
    if run.spec.first_row_titles {
        if let Ok(headers) = reader.headers() {
            let mut titles: Vec<String> = headers.iter().map(str::to_string).collect();
            titles.push(shell.t("problem"));
            let _ = writer.write_record(&titles);
        }
    }
    for record in reader.records().flatten() {
        let line = record.position().map(|p| p.line()).unwrap_or(0);
        if !wanted.contains(&line) {
            continue;
        }
        let mut cells: Vec<String> = record.iter().map(str::to_string).collect();
        cells.push(
            problems
                .get(&line)
                .map(|m| m.join("; "))
                .unwrap_or_default(),
        );
        let _ = writer.write_record(&cells);
    }
    let body = writer
        .into_inner()
        .map(|b| format!("\u{feff}{}", String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    csv_download(&format!("refused-{}.csv", run.id), body)
}

/// A template rendered into the chrome under `title`.
fn page<T: Template>(shell: &Shell, title: &Text, body: &T) -> Response {
    match Markup::from_template(body) {
        Ok(body) => shell.page(title, body),
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}
