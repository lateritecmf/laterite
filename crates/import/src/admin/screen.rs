//! The screen: a resource chosen, a file taken in, its columns matched, the run
//! started and watched.

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use futures::TryStreamExt;
use laterite_admin::axum::extract::{Form, Path, Query, State};
use laterite_admin::axum::response::{Html, IntoResponse, Redirect, Response};
use laterite_admin::axum::routing::get;
use laterite_admin::axum::{Extension, Router};
use laterite_admin::form::FormHandle;
use laterite_admin::html::Markup;
use laterite_admin::routes::{RouteCtx, Screen};
use laterite_admin::{AdminError, FlashLevel, SessionHandle, Shell, VerifiedUpload};
use laterite_auth::AuthenticatedUser;
use laterite_core::validation::Rule;
use laterite_core::{t, Actor, Db, Text};
use laterite_media::{blob_path, ingest, record, stream_of, DiskReg, NewMedia, StorageDriver};
use serde::Deserialize;

use super::{ImportReg, DISK};
use crate::file::{peek, PEEK_BYTES};
use crate::run::{find, start, Duplicates, Match, Note, NoteKind, Run, RunError, RunSpec, Status};
use crate::store;

/// The largest file the screen takes in.
const UPLOAD_LIMIT: u64 = 20 * 1024 * 1024;

/// How many runs the screen lists.
const RECENT_RUNS: i64 = 20;

/// The screen, registered by the module.
pub(crate) struct ImportScreen;

/// One resource the screen imports into, resolved at mount.
struct Target {
    /// The resource path without its leading slash, as it appears in URLs.
    key: String,
    resource: String,
    label: Text,
    permission: Option<String>,
    form: FormHandle,
}

/// What the handlers need after mount, cloned per request.
#[derive(Clone)]
struct App {
    db: Db,
    base: Arc<str>,
    disk: Arc<dyn StorageDriver>,
    targets: Arc<Vec<Target>>,
}

impl App {
    fn url(&self, relative: &str) -> String {
        format!(
            "{}/{}",
            self.base.trim_end_matches('/'),
            relative.trim_start_matches('/')
        )
    }

    /// The target behind `key`, when there is one and `user` may use it.
    fn target(&self, key: &str, user: &AuthenticatedUser) -> Result<&Target, AdminError> {
        let target = self
            .targets
            .iter()
            .find(|t| t.key == key)
            .ok_or(AdminError::NotFound)?;
        match &target.permission {
            Some(code) if !user.permissions.allows(code) => Err(AdminError::Forbidden),
            _ => Ok(target),
        }
    }

    /// The targets `user` may use, as the index lists them.
    fn allowed(&self, user: &AuthenticatedUser) -> Vec<&Target> {
        self.targets
            .iter()
            .filter(|t| {
                t.permission
                    .as_deref()
                    .is_none_or(|code| user.permissions.allows(code))
            })
            .collect()
    }

    fn label_of(&self, resource: &str, shell: &Shell) -> String {
        self.targets
            .iter()
            .find(|t| t.resource == resource)
            .map(|t| shell.tt(&t.label))
            .unwrap_or_else(|| resource.to_string())
    }
}

impl Screen for ImportScreen {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let disk = ctx
            .contributions::<DiskReg>()
            .into_iter()
            .find(|d| d.name == DISK)
            .map(|d| d.driver.clone())
            .unwrap_or_else(|| {
                panic!(
                    "laterite.import stores uploads on a disk named `{DISK}`; register a \
                     laterite_media::DiskReg with that name"
                )
            });
        let targets = ctx
            .contributions::<ImportReg>()
            .into_iter()
            .map(|reg| Target {
                key: reg.resource.trim_start_matches('/').to_string(),
                resource: reg.resource.clone(),
                label: reg.label.clone(),
                permission: reg.permission.clone(),
                form: ctx.form(&reg.resource).unwrap_or_else(|| {
                    panic!(
                        "import is registered for `{}`, which mounts no form",
                        reg.resource
                    )
                }),
            })
            .collect();
        let app = App {
            db: ctx.db().clone(),
            base: Arc::from(ctx.base_path()),
            disk,
            targets: Arc::new(targets),
        };
        Router::new()
            .route("/", get(index))
            .route("/upload/{*key}", get(upload_form).post(upload))
            .route("/match/{media}/{*key}", get(match_form).post(start_run))
            .route("/runs/{id}", get(run_page))
            .route("/runs/{id}/status", get(run_status))
            .with_state(app)
    }
}

// The index: what may be imported into, and the latest runs.

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    shell: &'a Shell,
    targets: Vec<TargetRow>,
    runs: Vec<RunRow>,
}

struct TargetRow {
    label: String,
    href: String,
}

struct RunRow {
    id: i64,
    href: String,
    resource: String,
    status: String,
    status_class: &'static str,
    created: i64,
    updated: i64,
    skipped: i64,
    errors: i64,
    started_at: String,
}

async fn index(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    let targets = app
        .allowed(&user)
        .into_iter()
        .map(|t| TargetRow {
            label: shell.tt(&t.label),
            href: app.url(&format!("upload/{}", t.key)),
        })
        .collect();
    let runs = match store::recent(&app.db, RECENT_RUNS).await {
        Ok(runs) => runs,
        Err(e) => return AdminError::Internal(e.into()).into_response(),
    };
    let runs = runs
        .into_iter()
        .map(|run| RunRow {
            id: run.id,
            href: app.url(&format!("runs/{}", run.id)),
            resource: app.label_of(&run.resource, &shell),
            status: status_label(&shell, run.status),
            status_class: status_class(run.status),
            created: run.created,
            updated: run.updated,
            skipped: run.skipped,
            errors: run.errors,
            started_at: shell.datetime(&run.started_at),
        })
        .collect();
    page(
        &shell,
        &t!("Import"),
        &IndexTemplate {
            shell: &shell,
            targets,
            runs,
        },
    )
}

// The upload: a file and how to read it.

#[derive(Template)]
#[template(path = "upload.html")]
struct UploadTemplate<'a> {
    shell: &'a Shell,
    action: String,
    back: String,
}

async fn upload_form(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(key): Path<String>,
) -> Response {
    let target = match app.target(&key, &user) {
        Ok(target) => target,
        Err(e) => return e.into_response(),
    };
    page(
        &shell,
        &t!("Import into {resource}", resource = shell.tt(&target.label)),
        &UploadTemplate {
            shell: &shell,
            action: app.url(&format!("upload/{}", target.key)),
            back: app.url(""),
        },
    )
}

/// How the file is read: carried from the upload to the matching step and on
/// to the run.
#[derive(Debug, Clone, Deserialize)]
struct Reading {
    #[serde(default = "titles_on")]
    titles: String,
    #[serde(default = "comma")]
    delimiter: String,
}

fn titles_on() -> String {
    "1".to_string()
}

fn comma() -> String {
    ",".to_string()
}

impl Reading {
    fn first_row_titles(&self) -> bool {
        self.titles == "1"
    }

    fn delimiter_byte(&self) -> u8 {
        match self.delimiter.as_str() {
            ";" => b';',
            "tab" => b'\t',
            _ => b',',
        }
    }

    fn query(&self) -> String {
        format!(
            "?titles={}&delimiter={}",
            if self.first_row_titles() { "1" } else { "0" },
            match self.delimiter_byte() {
                b';' => "%3B",
                b'\t' => "tab",
                _ => "%2C",
            }
        )
    }
}

async fn upload(
    State(app): State<App>,
    Extension(session): Extension<SessionHandle>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(key): Path<String>,
    mut upload: VerifiedUpload,
) -> Response {
    let target = match app.target(&key, &user) {
        Ok(target) => target,
        Err(e) => return e.into_response(),
    };
    let back = app.url(&format!("upload/{}", target.key));
    let mut reading = Reading {
        titles: "0".to_string(),
        delimiter: comma(),
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
        Ok(id) => Redirect::to(&format!(
            "{}{}",
            app.url(&format!("match/{id}/{}", target.key)),
            reading.query()
        ))
        .into_response(),
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}

// The matching step.

#[derive(Template)]
#[template(path = "match.html")]
struct MatchTemplate<'a> {
    shell: &'a Shell,
    action: String,
    back: String,
    file_name: String,
    titles: bool,
    delimiter: String,
    columns: Vec<ColumnRow>,
    fields: Vec<FieldRow>,
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

fn field_rows(form: &FormHandle, shell: &Shell) -> Vec<FieldRow> {
    form.fields()
        .iter()
        .map(|f| FieldRow {
            name: f.name.clone(),
            label: shell.tt(&f.label),
            required: f.rules.iter().any(|r| matches!(r, Rule::Required)),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn render_match(
    app: &App,
    shell: &Shell,
    target: &Target,
    record: &record::MediaRecord,
    reading: &Reading,
    chosen: &HashMap<String, String>,
    error: Option<String>,
) -> Response {
    let bytes = match head_of(app, record).await {
        Ok(bytes) => bytes,
        Err(e) => return e.into_response(),
    };
    let look = peek(&bytes, reading.delimiter_byte(), reading.first_row_titles());
    let fields = field_rows(&target.form, shell);
    let columns = look
        .titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let key = format!("match_{index}");
            // A field already chosen stays chosen; otherwise the field whose name
            // or label reads like the title is proposed.
            let selected = chosen.get(&key).cloned().unwrap_or_else(|| {
                let want = folded(title);
                fields
                    .iter()
                    .find(|f| folded(&f.name) == want || folded(&f.label) == want)
                    .map(|f| f.name.clone())
                    .unwrap_or_default()
            });
            ColumnRow {
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
                selected,
            }
        })
        .collect();
    let action = app.url(&format!("match/{}/{}", record.id, target.key));
    page(
        shell,
        &t!("Import into {resource}", resource = shell.tt(&target.label)),
        &MatchTemplate {
            shell,
            action,
            back: app.url(&format!("upload/{}", target.key)),
            file_name: record.original_name.clone(),
            titles: reading.first_row_titles(),
            delimiter: reading.delimiter.clone(),
            columns,
            fields,
            error,
        },
    )
}

async fn match_form(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((media, key)): Path<(i64, String)>,
    Query(reading): Query<Reading>,
) -> Response {
    let target = match app.target(&key, &user) {
        Ok(target) => target,
        Err(e) => return e.into_response(),
    };
    let record = match payload(&app, media, &user).await {
        Ok(record) => record,
        Err(e) => return e.into_response(),
    };
    render_match(
        &app,
        &shell,
        target,
        &record,
        &reading,
        &HashMap::new(),
        None,
    )
    .await
}

async fn start_run(
    State(app): State<App>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((media, key)): Path<(i64, String)>,
    Form(data): Form<HashMap<String, String>>,
) -> Response {
    let target = match app.target(&key, &user) {
        Ok(target) => target,
        Err(e) => return e.into_response(),
    };
    let record = match payload(&app, media, &user).await {
        Ok(record) => record,
        Err(e) => return e.into_response(),
    };
    let reading = Reading {
        titles: data.get("titles").cloned().unwrap_or_else(titles_on),
        delimiter: data.get("delimiter").cloned().unwrap_or_else(comma),
    };
    let refused = |error: Text| {
        let message = shell.tt(&error);
        render_match(
            &app,
            &shell,
            target,
            &record,
            &reading,
            &data,
            Some(message),
        )
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
    let fields = field_rows(&target.form, &shell);
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
    let mut spec = RunSpec::new(&target.resource, matches)
        .media(record.id)
        .first_row_titles(reading.first_row_titles())
        .delimiter(reading.delimiter_byte());
    if let Some(key) = data.get("key").filter(|k| !k.is_empty()) {
        if !spec.matches.iter().any(|m| &m.field == key) {
            return refused(t!(
                "The field records are matched on must itself be matched to a column."
            ))
            .await;
        }
        spec = spec.duplicates(match data.get("on_match").map(String::as_str) {
            Some("update") => Duplicates::update(key),
            _ => Duplicates::skip(key),
        });
    }
    let source = match app.disk.get(&blob_path(&record.hash)).await {
        Ok(source) => source,
        Err(e) => return AdminError::Internal(e.into()).into_response(),
    };
    match start(
        &app.db,
        target.form.clone(),
        Actor::from(&user),
        spec,
        source,
    )
    .await
    {
        Ok(id) => Redirect::to(&app.url(&format!("runs/{id}"))).into_response(),
        Err(RunError::Spec(reason)) => refused(Text::dynamic(reason)).await,
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}

// The report.

#[derive(Template)]
#[template(path = "run.html")]
struct RunTemplate<'a> {
    shell: &'a Shell,
    resource: String,
    actor: String,
    started_at: String,
    status: Markup,
    back: String,
}

#[derive(Template)]
#[template(path = "_run_status.html")]
struct StatusTemplate<'a> {
    shell: &'a Shell,
    poll: Option<String>,
    status: String,
    status_class: &'static str,
    run: &'a Run,
    failure: Option<String>,
    finished_at: Option<String>,
    notes: Vec<NoteRow>,
}

struct NoteRow {
    row: u64,
    kind: String,
    field: String,
    message: String,
}

fn status_label(shell: &Shell, status: Status) -> String {
    match status {
        Status::Running => shell.t("Running"),
        Status::Done => shell.t("Done"),
        Status::Failed => shell.t("Failed"),
    }
}

fn status_class(status: Status) -> &'static str {
    match status {
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
    Markup::from_template(&StatusTemplate {
        shell,
        poll: (run.status == Status::Running).then(|| app.url(&format!("runs/{}/status", run.id))),
        status: status_label(shell, run.status),
        status_class: status_class(run.status),
        run,
        failure: run.failure.clone(),
        finished_at: run.finished_at.as_deref().map(|at| shell.datetime(at)),
        notes,
    })
}

async fn load_run(app: &App, id: i64) -> Result<Run, AdminError> {
    find(&app.db, id)
        .await
        .map_err(|e| AdminError::Internal(e.into()))?
        .ok_or(AdminError::NotFound)
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
    page(
        &shell,
        &t!("Import run {id}", id = run.id),
        &RunTemplate {
            shell: &shell,
            resource: app.label_of(&run.resource, &shell),
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

/// A template rendered into the chrome under `title`.
fn page<T: Template>(shell: &Shell, title: &Text, body: &T) -> Response {
    match Markup::from_template(body) {
        Ok(body) => shell.page(title, body),
        Err(e) => AdminError::Internal(e.into()).into_response(),
    }
}
