//! Records kept outside the database: one file per record in a folder of
//! the project, or a store a module registers, pulled into the entity's
//! table at boot and written back on every save.
//!
//! A resource declares a [`Source`]. The admin reads and writes the entity's
//! table as it does any other, and a [`RecordSource`] keeps the store in
//! step: `pull` reads it into rows, `push` writes a saved row out, `remove`
//! takes a deleted row away. `files` (the folder) and `git` (the folder,
//! committed on every write) are built in; a module registers another as a
//! [`RecordSourceReg`] and a resource names it by `kind`.
//!
//! A file's name is the record's key column (`events/friday-night.yaml`
//! has `slug = "friday-night"`), its front matter or document is the
//! record's columns, and with the `markdown` format one column is the body
//! under the front matter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sea_query::{Alias, Expr, Query, SimpleExpr};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::listeners::{CREATED_AT, UPDATED_AT};
use crate::query::{bind_values, build, text_cast, AnyRowExt};
use crate::record::{Actor, AttrValue, ModelListener, Op, Record, SaveCx, SavedCx};
use crate::validation::ErrorBag;
use crate::{Db, Text};

/// Where a resource's records live, as its descriptor declares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// `files` writes the folder, `git` also commits every write, or the
    /// code of a source a module registered. Default `files`.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// The folder, relative to the project root (`content/events`).
    pub path: String,
    /// The file format; also the extension.
    #[serde(default)]
    pub format: Format,
    /// The text column that names the file. Default `slug`.
    #[serde(default = "default_key")]
    pub key: String,
    /// With `markdown`, the column the body under the front matter holds.
    #[serde(default)]
    pub body: Option<String>,
    /// A column the key is derived from when a save leaves it blank
    /// (`title`), through [`slugify`].
    #[serde(default)]
    pub slug_from: Option<String>,
    /// Any other key, for a registered source's own settings.
    #[serde(flatten)]
    pub options: Map<String, Value>,
}

impl Source {
    /// A folder source of the format, keyed by `slug`.
    pub fn files(path: impl Into<String>, format: Format) -> Self {
        Self {
            kind: FILES.to_string(),
            path: path.into(),
            format,
            key: default_key(),
            body: None,
            slug_from: None,
            options: Map::new(),
        }
    }

    /// The same, committed on every write.
    pub fn git(path: impl Into<String>, format: Format) -> Self {
        Self {
            kind: GIT.to_string(),
            ..Self::files(path, format)
        }
    }

    /// Names the body column (`markdown`).
    pub fn body(mut self, column: impl Into<String>) -> Self {
        self.body = Some(column.into());
        self
    }

    /// Names the column the key is derived from when blank.
    pub fn slug_from(mut self, column: impl Into<String>) -> Self {
        self.slug_from = Some(column.into());
        self
    }
}

fn default_key() -> String {
    "slug".to_string()
}

fn default_kind() -> String {
    FILES.to_string()
}

/// The built-in source that writes the folder.
pub const FILES: &str = "files";
/// The built-in source that writes the folder and commits every write.
pub const GIT: &str = "git";

/// How a record is spelled in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// A YAML document of the columns.
    #[default]
    Yaml,
    /// A JSON object of the columns.
    Json,
    /// YAML front matter between `---` lines, then the body column.
    Markdown,
}

impl Format {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Yaml => "yaml",
            Format::Json => "json",
            Format::Markdown => "md",
        }
    }
}

/// How a column's text is spelled in a file: a stored `1` is `true` for a
/// switch, `3` is a number, and a repeater's JSON is structured data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    /// Written as a string.
    #[default]
    Text,
    /// Written as a boolean: a stored `1` is `true`.
    Bool,
    /// Written as a number.
    Number,
    /// Written as structured data parsed from the stored JSON.
    Json,
}

/// A stored column and its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Name of the column in the entity's table.
    pub name: String,
    /// How the column's text is spelled in a file.
    pub shape: Shape,
}

impl Column {
    /// Returns a column named `name` of the given shape.
    pub fn new(name: impl Into<String>, shape: Shape) -> Self {
        Self {
            name: name.into(),
            shape,
        }
    }
}

/// The table behind a sourced resource: its primary key and the columns its
/// form stores, in the form's order (which is the file's order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Columns {
    /// Name of the primary key column.
    pub id: String,
    /// Columns the form stores, in the file's order.
    pub stored: Vec<Column>,
    /// The table has `created_at` and `updated_at`: a pull stamps them, and
    /// the file never carries them.
    pub timestamps: bool,
}

impl Columns {
    /// Returns the columns of a table keyed by `id`, without timestamps.
    pub fn new(id: impl Into<String>, stored: Vec<Column>) -> Self {
        Self {
            id: id.into(),
            stored,
            timestamps: false,
        }
    }

    /// Marks the table as stamped (`created_at`, `updated_at`).
    pub fn timestamps(mut self) -> Self {
        self.timestamps = true;
        self
    }

    fn has(&self, name: &str) -> bool {
        self.stored.iter().any(|c| c.name == name)
    }
}

/// What went wrong reading or writing a source.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The declaration does not fit the resource.
    #[error("{0}")]
    Config(String),
    /// A file could not be read, parsed or written.
    #[error("{path}: {message}")]
    File {
        /// Path of the file.
        path: PathBuf,
        /// What went wrong with it.
        message: String,
    },
    /// Produced when the database reports an error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// `git` refused, or is not installed.
    #[error("git: {0}")]
    Git(String),
}

/// What a pull did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullReport {
    /// How many rows were inserted for new records.
    pub created: usize,
    /// How many existing rows were updated from their records.
    pub updated: usize,
    /// How many rows were deleted because their record is gone.
    pub removed: usize,
}

/// One file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The record's key, from the file name without its extension.
    pub key: String,
    /// The columns the file spells, by name.
    pub attrs: Map<String, Value>,
    /// The body under the front matter, for the `markdown` format.
    pub body: Option<String>,
}

impl Source {
    /// Checks the declaration against the table: the folder stays inside the
    /// project, the key and body name stored columns, and the body is only
    /// for `markdown`.
    pub fn validate(&self, columns: &Columns) -> Result<(), SourceError> {
        let path = Path::new(&self.path);
        if self.path.trim().is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(SourceError::Config(format!(
                "source.path `{}` must be a folder inside the project",
                self.path
            )));
        }
        if !columns.has(&self.key) {
            return Err(SourceError::Config(format!(
                "source.key `{}` is not a stored field of the form",
                self.key
            )));
        }
        match (&self.body, self.format) {
            (Some(body), Format::Markdown) if !columns.has(body) => {
                return Err(SourceError::Config(format!(
                    "source.body `{body}` is not a stored field of the form"
                )));
            }
            (Some(body), Format::Markdown) if body == &self.key => {
                return Err(SourceError::Config(
                    "source.body cannot be the key column".to_string(),
                ));
            }
            (Some(_), Format::Markdown) | (None, _) => {}
            (Some(_), _) => {
                return Err(SourceError::Config(
                    "source.body is only read with format = \"markdown\"".to_string(),
                ));
            }
        }
        if let Some(from) = &self.slug_from {
            if !columns.has(from) {
                return Err(SourceError::Config(format!(
                    "source.slug_from `{from}` is not a stored field of the form"
                )));
            }
        }
        Ok(())
    }

    /// The folder, under the project root.
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(&self.path)
    }

    /// The file a key names.
    pub fn file_name(&self, key: &str) -> String {
        format!("{key}.{}", self.format.extension())
    }
}

/// What a source is given to work with.
pub struct SourceCx<'a> {
    /// The database the table lives in.
    pub db: &'a Db,
    /// The project root, the directory holding `config/`.
    pub root: &'a Path,
    /// The entity's table.
    pub entity: &'a str,
    /// The table's columns.
    pub columns: &'a Columns,
    /// The declaration.
    pub source: &'a Source,
    /// Who is writing; `None` on a pull.
    pub actor: Option<&'a Actor>,
}

impl SourceCx<'_> {
    /// The folder, under the project root.
    pub fn dir(&self) -> PathBuf {
        self.source.dir(self.root)
    }

    /// The row with `id` as its file spells it: the key, the columns by
    /// shape, and the body. `None` when the row is gone or has no usable key.
    pub async fn row(&self, id: i64) -> Result<Option<Row>, SourceError> {
        row_values(self.db, self.entity, self.columns, self.source, id).await
    }
}

/// A row as a source writes it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The record's key, which names its file.
    pub key: String,
    /// The columns other than the key and body, spelled by shape.
    pub attrs: Map<String, Value>,
    /// The body column's text, when the source declares one and it is set.
    pub body: Option<String>,
}

/// A saved record, for a source to write out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    /// The row's id.
    pub id: i64,
    /// Whether it was created or updated.
    pub op: Op,
    /// The key before the save, when the save changed it: the source retires
    /// what it kept under that key.
    pub previous_key: Option<String>,
}

/// A place records live outside the database.
#[async_trait]
pub trait RecordSource: Send + Sync {
    /// Reads the store into the table: a record is created or updated by
    /// key, and a row whose record is gone is removed.
    async fn pull(&self, cx: &SourceCx<'_>) -> Result<PullReport, SourceError>;
    /// Writes one saved row out.
    async fn push(&self, cx: &SourceCx<'_>, pushed: &Pushed) -> Result<(), SourceError>;
    /// Takes one deleted row's record away.
    async fn remove(&self, cx: &SourceCx<'_>, key: &str) -> Result<(), SourceError>;
}

/// A source a module contributes, chosen by a resource's `kind`.
pub struct RecordSourceReg {
    /// The code a resource's `kind` selects this source by.
    pub code: String,
    /// The source itself.
    pub source: Arc<dyn RecordSource>,
}

impl RecordSourceReg {
    /// Returns a registration offering `source` under `code`.
    pub fn new(code: impl Into<String>, source: Arc<dyn RecordSource>) -> Self {
        Self {
            code: code.into(),
            source,
        }
    }
}

/// The source a code names: `files`, `git`, or one of the registered.
pub fn resolve(code: &str, registered: &[RecordSourceReg]) -> Option<Arc<dyn RecordSource>> {
    match code {
        FILES => Some(Arc::new(Files)),
        GIT => Some(Arc::new(Git)),
        other => registered
            .iter()
            .find(|r| r.code == other)
            .map(|r| Arc::clone(&r.source)),
    }
}

/// Whether a key can name a file: letters, digits, dots, dashes and
/// underscores, not starting with a dot.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('.')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Lowercase ASCII letters and digits with single dashes between words:
/// `"Friday Night, Live!"` is `friday-night-live`.
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Reads one file's text into its columns and body.
pub fn parse(format: Format, text: &str) -> Result<(Map<String, Value>, Option<String>), String> {
    let text = text.trim_start_matches('\u{feff}');
    match format {
        Format::Yaml => {
            if text.trim().is_empty() {
                return Ok((Map::new(), None));
            }
            let value: Value = serde_saphyr::from_str(text).map_err(|e| e.to_string())?;
            object(value).map(|m| (m, None))
        }
        Format::Json => {
            let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
            object(value).map(|m| (m, None))
        }
        Format::Markdown => {
            let Some(rest) = text
                .strip_prefix("---\n")
                .or_else(|| text.strip_prefix("---\r\n"))
            else {
                return Ok((Map::new(), Some(text.to_string())));
            };
            let (front, body) = match rest.split_once("\n---\n") {
                Some(parts) => parts,
                None => match rest.split_once("\n---\r\n") {
                    Some(parts) => parts,
                    None => (rest.trim_end_matches(['-', '\n', '\r']), ""),
                },
            };
            let attrs = if front.trim().is_empty() {
                Map::new()
            } else {
                let value: Value = serde_saphyr::from_str(front).map_err(|e| e.to_string())?;
                object(value)?
            };
            let body = body.strip_prefix('\n').unwrap_or(body);
            Ok((attrs, Some(body.trim_end_matches('\n').to_string())))
        }
    }
}

fn object(value: Value) -> Result<Map<String, Value>, String> {
    match value {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(Map::new()),
        _ => Err("the document is not a map of fields".to_string()),
    }
}

/// Spells columns and a body as one file's text.
pub fn render(format: Format, attrs: &Map<String, Value>, body: Option<&str>) -> String {
    match format {
        Format::Yaml => yaml(attrs),
        Format::Json => {
            let mut text = serde_json::to_string_pretty(attrs).unwrap_or_else(|_| "{}".into());
            text.push('\n');
            text
        }
        Format::Markdown => {
            let body = body.unwrap_or_default();
            format!("---\n{}---\n{body}\n", yaml(attrs))
        }
    }
}

fn yaml(attrs: &Map<String, Value>) -> String {
    if attrs.is_empty() {
        return String::new();
    }
    let mut text = serde_saphyr::to_string(attrs).unwrap_or_default();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Reads every file of the format in the folder, by key. A missing folder
/// is empty.
pub fn read_dir(dir: &Path, format: Format) -> Result<Vec<Entry>, SourceError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let suffix = format!(".{}", format.extension());
    let mut entries = Vec::new();
    let listing = std::fs::read_dir(dir).map_err(|e| SourceError::File {
        path: dir.to_path_buf(),
        message: e.to_string(),
    })?;
    for item in listing {
        let item = item.map_err(|e| SourceError::File {
            path: dir.to_path_buf(),
            message: e.to_string(),
        })?;
        let path = item.path();
        let name = item.file_name().to_string_lossy().to_string();
        let Some(key) = name.strip_suffix(&suffix) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        if !valid_key(key) {
            return Err(SourceError::File {
                path,
                message: "the file name must be letters, digits, dots, dashes and underscores"
                    .to_string(),
            });
        }
        let text = std::fs::read_to_string(&path).map_err(|e| SourceError::File {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let (attrs, body) = parse(format, &text).map_err(|message| SourceError::File {
            path: path.clone(),
            message,
        })?;
        entries.push(Entry {
            key: key.to_string(),
            attrs,
            body,
        });
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(entries)
}

/// A file's value as the attribute the table stores.
fn attr(value: &Value) -> AttrValue {
    match value {
        Value::Null => AttrValue::Null,
        Value::Bool(b) => AttrValue::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => AttrValue::Int(i),
            None => AttrValue::Float(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => AttrValue::Text(s.clone()),
        other => AttrValue::Json(other.clone()),
    }
}

fn expr(value: &AttrValue) -> SimpleExpr {
    match value {
        AttrValue::Null => SimpleExpr::Keyword(sea_query::Keyword::Null),
        AttrValue::Int(i) => (*i).into(),
        AttrValue::Float(f) => (*f).into(),
        AttrValue::Bool(b) => i32::from(*b).into(),
        AttrValue::Text(s) if s.is_empty() => SimpleExpr::Keyword(sea_query::Keyword::Null),
        other => other.to_text().unwrap_or_default().into(),
    }
}

/// A stored text as the file spells it, by the column's shape.
fn spell(text: &str, shape: Shape) -> Value {
    match shape {
        Shape::Text => Value::String(text.to_string()),
        Shape::Bool => Value::Bool(matches!(text, "1" | "true" | "on")),
        Shape::Number => text
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| text.parse::<f64>().map(Value::from))
            .unwrap_or_else(|_| Value::String(text.to_string())),
        Shape::Json => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
        }
    }
}

/// The record a file holds, in the table's columns: every stored column
/// the file names, the body column from the body. A key the form does not
/// know is refused, since a save would drop it.
fn record_from(
    entity: &str,
    columns: &Columns,
    source: &Source,
    entry: &Entry,
    path: &Path,
) -> Result<Record, SourceError> {
    let mut rec = Record::new(entity);
    for (name, value) in &entry.attrs {
        if name == &columns.id {
            continue;
        }
        if !columns.has(name) || source.body.as_deref() == Some(name) {
            return Err(SourceError::File {
                path: path.to_path_buf(),
                message: format!("`{name}` is not a field of {entity}"),
            });
        }
        rec.set(name.clone(), attr(value));
    }
    // A switch the file leaves out is off: the stored column is never NULL
    // for it, so a pull does not fail a NOT NULL constraint on an update.
    for column in &columns.stored {
        if column.shape == Shape::Bool && !rec.contains(&column.name) {
            rec.set(column.name.clone(), false);
        }
    }
    rec.set(source.key.clone(), entry.key.clone());
    if let Some(body) = &source.body {
        rec.set(body.clone(), entry.body.clone().unwrap_or_default());
    }
    Ok(rec)
}

/// Reads the folder into the table: a file's row is created or updated by
/// key, and a row whose file is gone is removed. The files are the truth.
/// What the `files` source does on pull, for a source built on a folder.
pub async fn pull(cx: &SourceCx<'_>) -> Result<PullReport, SourceError> {
    let SourceCx {
        db,
        entity,
        columns,
        source,
        ..
    } = *cx;
    let dir = cx.dir();
    let entries = read_dir(&dir, source.format)?;
    let existing = keys(db, entity, columns, source).await?;
    let mut report = PullReport::default();
    let mut seen = std::collections::HashSet::new();
    for entry in &entries {
        let path = dir.join(source.file_name(&entry.key));
        let rec = record_from(entity, columns, source, entry, &path)?;
        seen.insert(entry.key.clone());
        match existing.get(&entry.key) {
            Some(id) => {
                let (sql, values) = update_sql(db, entity, columns, &rec, *id);
                bind_values(sqlx::query(&sql), values)
                    .execute(&db.pool)
                    .await?;
                report.updated += 1;
            }
            None => {
                let (sql, bound) = insert_sql(db, entity, columns, &rec);
                bind_values(sqlx::query(&sql), bound)
                    .execute(&db.pool)
                    .await?;
                report.created += 1;
            }
        }
    }
    let gone: Vec<i64> = existing
        .iter()
        .filter(|(key, _)| !seen.contains(*key))
        .map(|(_, id)| *id)
        .collect();
    if !gone.is_empty() {
        let (sql, values) = delete_sql(db, entity, columns, &gone);
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
        report.removed = gone.len();
    }
    Ok(report)
}

// The builders are scoped in these helpers: sea-query identifiers are
// reference-counted (not `Send`), and one alive across an await would make
// the listener's future non-`Send`.
fn update_sql(
    db: &Db,
    entity: &str,
    columns: &Columns,
    rec: &Record,
    id: i64,
) -> (String, sea_query::Values) {
    let mut stmt = Query::update();
    stmt.table(Alias::new(entity));
    for column in &columns.stored {
        stmt.value(
            Alias::new(&column.name),
            expr(rec.get(&column.name).unwrap_or(&AttrValue::Null)),
        );
    }
    if columns.timestamps {
        stmt.value(Alias::new(UPDATED_AT), expr(&now()));
    }
    stmt.and_where(Expr::col(Alias::new(&columns.id)).eq(id));
    build(db.backend, stmt)
}

fn insert_sql(
    db: &Db,
    entity: &str,
    columns: &Columns,
    rec: &Record,
) -> (String, sea_query::Values) {
    let (mut names, mut values): (Vec<_>, Vec<_>) = columns
        .stored
        .iter()
        .filter_map(|c| {
            rec.get(&c.name)
                .filter(|v| !v.is_null())
                .map(|v| (Alias::new(&c.name), expr(v)))
        })
        .unzip();
    if columns.timestamps {
        let stamp = now();
        names.push(Alias::new(CREATED_AT));
        values.push(expr(&stamp));
        names.push(Alias::new(UPDATED_AT));
        values.push(expr(&stamp));
    }
    build(
        db.backend,
        Query::insert()
            .into_table(Alias::new(entity))
            .columns(names)
            .values_panic(values)
            .to_owned(),
    )
}

fn now() -> AttrValue {
    AttrValue::DateTime(chrono::Utc::now())
}

fn delete_sql(
    db: &Db,
    entity: &str,
    columns: &Columns,
    ids: &[i64],
) -> (String, sea_query::Values) {
    build(
        db.backend,
        Query::delete()
            .from_table(Alias::new(entity))
            .and_where(Expr::col(Alias::new(&columns.id)).is_in(ids.iter().copied()))
            .to_owned(),
    )
}

fn keys_sql(
    db: &Db,
    entity: &str,
    columns: &Columns,
    source: &Source,
) -> (String, sea_query::Values) {
    build(
        db.backend,
        Query::select()
            .column(Alias::new(&columns.id))
            .expr_as(
                Expr::col(Alias::new(&source.key)).cast_as(Alias::new(text_cast(db.backend))),
                Alias::new("source_key"),
            )
            .from(Alias::new(entity))
            .to_owned(),
    )
}

fn row_sql(db: &Db, entity: &str, columns: &Columns, id: i64) -> (String, sea_query::Values) {
    let cast = text_cast(db.backend);
    let mut select = Query::select();
    for column in &columns.stored {
        select.expr_as(
            Expr::col(Alias::new(&column.name)).cast_as(Alias::new(cast)),
            Alias::new(&column.name),
        );
    }
    select
        .from(Alias::new(entity))
        .and_where(Expr::col(Alias::new(&columns.id)).eq(id));
    build(db.backend, select)
}

/// Every row's key and id.
async fn keys(
    db: &Db,
    entity: &str,
    columns: &Columns,
    source: &Source,
) -> Result<HashMap<String, i64>, SourceError> {
    let (sql, values) = keys_sql(db, entity, columns, source);
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut map = HashMap::new();
    for row in rows {
        let id = row.get_int(&columns.id)?;
        if let Some(key) = row.get_text_opt("source_key")? {
            map.insert(key, id);
        }
    }
    Ok(map)
}

/// One row as a source writes it: the key, the columns by shape, the body.
pub async fn row_values(
    db: &Db,
    entity: &str,
    columns: &Columns,
    source: &Source,
    id: i64,
) -> Result<Option<Row>, SourceError> {
    let (sql, values) = row_sql(db, entity, columns, id);
    let Some(row) = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?
    else {
        return Ok(None);
    };
    let mut attrs = Map::new();
    let mut body = None;
    let mut key = None;
    for column in &columns.stored {
        let Some(text) = row.get_text_opt(&column.name)? else {
            continue;
        };
        if column.name == source.key {
            key = Some(text);
        } else if source.body.as_deref() == Some(column.name.as_str()) {
            body = Some(text);
        } else {
            attrs.insert(column.name.clone(), spell(&text, column.shape));
        }
    }
    let Some(key) = key.filter(|k| valid_key(k)) else {
        return Ok(None);
    };
    Ok(Some(Row { key, attrs, body }))
}

/// The `files` source: the folder is written, one file per record.
pub struct Files;

impl Files {
    fn write(cx: &SourceCx<'_>, key: &str, text: &str) -> Result<(), SourceError> {
        let dir = cx.dir();
        std::fs::create_dir_all(&dir).map_err(|e| SourceError::File {
            path: dir.clone(),
            message: e.to_string(),
        })?;
        let path = dir.join(cx.source.file_name(key));
        std::fs::write(&path, text).map_err(|e| SourceError::File {
            path,
            message: e.to_string(),
        })
    }

    fn remove_file(cx: &SourceCx<'_>, key: &str) -> Result<(), SourceError> {
        let path = cx.dir().join(cx.source.file_name(key));
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| SourceError::File {
                path,
                message: e.to_string(),
            })?;
        }
        Ok(())
    }
}

#[async_trait]
impl RecordSource for Files {
    async fn pull(&self, cx: &SourceCx<'_>) -> Result<PullReport, SourceError> {
        pull(cx).await
    }

    async fn push(&self, cx: &SourceCx<'_>, pushed: &Pushed) -> Result<(), SourceError> {
        let Some(row) = cx.row(pushed.id).await? else {
            return Ok(());
        };
        if let Some(previous) = pushed.previous_key.as_deref() {
            if previous != row.key && valid_key(previous) {
                Self::remove_file(cx, previous)?;
            }
        }
        let text = render(cx.source.format, &row.attrs, row.body.as_deref());
        Self::write(cx, &row.key, &text)
    }

    async fn remove(&self, cx: &SourceCx<'_>, key: &str) -> Result<(), SourceError> {
        Self::remove_file(cx, key)
    }
}

/// The `git` source: the folder, committed on every write with the
/// operator named in the message. Runs `git` in the project root.
pub struct Git;

impl Git {
    fn commit(cx: &SourceCx<'_>, message: &str) -> Result<(), SourceError> {
        let message = match cx.actor {
            Some(actor) => format!("{message} ({})", actor.label()),
            None => message.to_string(),
        };
        commit_dir(cx.root, &cx.dir(), &message).map(|_| ())
    }
}

/// Stages everything under `dir` and commits it with `message` in the
/// repository at `root`, as the `git` source does for a record's folder.
/// `Ok(false)` when nothing under `dir` had changed.
pub fn commit_dir(root: &Path, dir: &Path, message: &str) -> Result<bool, SourceError> {
    let dir = dir.to_string_lossy().to_string();
    git(root, &["add", "-A", "--", &dir])?;
    let staged = git(root, &["diff", "--cached", "--quiet", "--", &dir]).is_err();
    if staged {
        git(root, &["commit", "-q", "-m", message, "--", &dir])?;
    }
    Ok(staged)
}

#[async_trait]
impl RecordSource for Git {
    async fn pull(&self, cx: &SourceCx<'_>) -> Result<PullReport, SourceError> {
        pull(cx).await
    }

    async fn push(&self, cx: &SourceCx<'_>, pushed: &Pushed) -> Result<(), SourceError> {
        Files.push(cx, pushed).await?;
        let Some(row) = cx.row(pushed.id).await? else {
            return Ok(());
        };
        let verb = if pushed.op == Op::Create {
            "Added"
        } else {
            "Updated"
        };
        Self::commit(
            cx,
            &format!(
                "{verb} {}/{}",
                cx.source.path,
                cx.source.file_name(&row.key)
            ),
        )
    }

    async fn remove(&self, cx: &SourceCx<'_>, key: &str) -> Result<(), SourceError> {
        Files.remove(cx, key).await?;
        Self::commit(
            cx,
            &format!("Removed {}/{}", cx.source.path, cx.source.file_name(key)),
        )
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, SourceError> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| SourceError::Git(format!("could not run git: {e}")))?;
    if !out.status.success() {
        return Err(SourceError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Keeps a sourced entity's store in step with its table: a saved row is
/// pushed and a deleted row removed, after the write commits. Before a
/// save, the key is derived and checked.
pub struct SourceListener {
    root: PathBuf,
    entity: String,
    columns: Columns,
    source: Source,
    driver: Arc<dyn RecordSource>,
}

impl SourceListener {
    /// Returns a listener keeping `source` (driven by `driver`) in step with
    /// the `entity` table under the project `root`.
    pub fn new(
        root: impl Into<PathBuf>,
        entity: impl Into<String>,
        columns: Columns,
        source: Source,
        driver: Arc<dyn RecordSource>,
    ) -> Self {
        Self {
            root: root.into(),
            entity: entity.into(),
            columns,
            source,
            driver,
        }
    }

    fn cx<'a>(&'a self, db: &'a Db, actor: Option<&'a Actor>) -> SourceCx<'a> {
        SourceCx {
            db,
            root: &self.root,
            entity: &self.entity,
            columns: &self.columns,
            source: &self.source,
            actor,
        }
    }
}

#[async_trait]
impl ModelListener for SourceListener {
    async fn before_save(
        &self,
        _cx: &mut SaveCx<'_>,
        rec: &mut Record,
        _op: Op,
    ) -> Result<(), ErrorBag> {
        let key = rec
            .text(&self.source.key)
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string);
        let key = match (key, &self.source.slug_from) {
            (Some(key), _) => key,
            (None, Some(from)) => slugify(rec.text(from).unwrap_or_default()),
            (None, None) => String::new(),
        };
        let mut errors = ErrorBag::default();
        if key.is_empty() {
            errors.add(
                &self.source.key,
                Text::new("This field names the file and is required."),
            );
        } else if !valid_key(&key) {
            errors.add(
                &self.source.key,
                Text::new("Use letters, digits, dots, dashes and underscores."),
            );
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        rec.set(self.source.key.clone(), key);
        Ok(())
    }

    async fn after_save(&self, cx: &SavedCx<'_>, rec: &Record, op: Op) {
        let Some(id) = rec.id() else {
            return;
        };
        let pushed = Pushed {
            id,
            op,
            previous_key: rec
                .original(&self.source.key)
                .and_then(AttrValue::as_str)
                .map(str::to_string),
        };
        let scx = self.cx(cx.db(), Some(cx.actor()));
        if let Err(e) = self.driver.push(&scx, &pushed).await {
            tracing::error!(entity = %self.entity, error = %e, "the record was not written to its source");
        }
    }

    async fn after_delete(&self, cx: &SavedCx<'_>, rec: &Record) {
        let Some(key) = rec.text(&self.source.key).filter(|k| valid_key(k)) else {
            return;
        };
        let scx = self.cx(cx.db(), Some(cx.actor()));
        if let Err(e) = self.driver.remove(&scx, key).await {
            tracing::error!(entity = %self.entity, error = %e, "the record was not removed from its source");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lowercase_ascii_with_single_dashes() {
        assert_eq!(slugify("Friday Night, Live!"), "friday-night-live");
        assert_eq!(slugify("  --Already-slug--  "), "already-slug");
        assert_eq!(slugify("Ünïcode café"), "n-code-caf");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn keys_are_file_names() {
        assert!(valid_key("friday-night"));
        assert!(valid_key("2026-11-05_wedding.v2"));
        assert!(!valid_key(".hidden"));
        assert!(!valid_key("a/b"));
        assert!(!valid_key(""));
        assert!(!valid_key("spa ce"));
    }

    #[test]
    fn yaml_and_json_round_trip() {
        let mut attrs = Map::new();
        attrs.insert("title".into(), Value::String("Friday".into()));
        attrs.insert("pinned".into(), Value::Bool(true));
        attrs.insert("capacity".into(), Value::from(120));
        for format in [Format::Yaml, Format::Json] {
            let text = render(format, &attrs, None);
            let (back, body) = parse(format, &text).unwrap();
            assert_eq!(back, attrs, "{format:?}: {text}");
            assert_eq!(body, None);
        }
    }

    #[test]
    fn markdown_keeps_front_matter_and_body() {
        let mut attrs = Map::new();
        attrs.insert("title".into(), Value::String("Hello".into()));
        let text = render(Format::Markdown, &attrs, Some("# Hello\n\nA body."));
        assert_eq!(text, "---\ntitle: Hello\n---\n# Hello\n\nA body.\n");
        let (back, body) = parse(Format::Markdown, &text).unwrap();
        assert_eq!(back, attrs);
        assert_eq!(body.as_deref(), Some("# Hello\n\nA body."));
    }

    #[test]
    fn markdown_without_front_matter_is_all_body() {
        let (attrs, body) = parse(Format::Markdown, "Just text\n").unwrap();
        assert!(attrs.is_empty());
        assert_eq!(body.as_deref(), Some("Just text\n"));
    }

    #[test]
    fn an_empty_yaml_file_is_an_empty_record() {
        let (attrs, body) = parse(Format::Yaml, "").unwrap();
        assert!(attrs.is_empty());
        assert_eq!(body, None);
        assert_eq!(render(Format::Yaml, &Map::new(), None), "");
    }

    #[test]
    fn a_document_that_is_not_a_map_is_refused() {
        assert!(parse(Format::Yaml, "- a\n- b\n").is_err());
        assert!(parse(Format::Json, "[1, 2]").is_err());
    }

    #[test]
    fn stored_text_is_spelled_by_shape() {
        assert_eq!(spell("1", Shape::Bool), Value::Bool(true));
        assert_eq!(spell("0", Shape::Bool), Value::Bool(false));
        assert_eq!(spell("12", Shape::Number), Value::from(12));
        assert_eq!(spell("1.5", Shape::Number), Value::from(1.5));
        assert_eq!(spell("x", Shape::Number), Value::String("x".into()));
        assert_eq!(
            spell(r#"[{"a":1}]"#, Shape::Json),
            serde_json::json!([{"a": 1}])
        );
        assert_eq!(spell("1", Shape::Text), Value::String("1".into()));
    }

    #[test]
    fn the_declaration_is_checked_against_the_columns() {
        let columns = Columns::new(
            "id",
            vec![
                Column::new("slug", Shape::Text),
                Column::new("title", Shape::Text),
                Column::new("body", Shape::Text),
            ],
        );
        let ok = Source::files("content/posts", Format::Markdown)
            .body("body")
            .slug_from("title");
        assert!(ok.validate(&columns).is_ok());
        let mut bad = ok.clone();
        bad.path = "../outside".into();
        assert!(bad.validate(&columns).is_err());
        let mut bad = ok.clone();
        bad.key = "missing".into();
        assert!(bad.validate(&columns).is_err());
        let mut bad = ok.clone();
        bad.format = Format::Yaml;
        assert!(bad.validate(&columns).is_err(), "a body needs markdown");
        let mut bad = ok.clone();
        bad.body = Some("slug".into());
        assert!(bad.validate(&columns).is_err());
    }

    #[test]
    fn a_source_reads_from_yaml_with_defaults() {
        let source: Source = serde_saphyr::from_str("path: content/events\n").unwrap();
        assert_eq!(source.kind, FILES);
        assert_eq!(source.format, Format::Yaml);
        assert_eq!(source.key, "slug");
        let source: Source = serde_saphyr::from_str(
            "kind: git\npath: content/posts\nformat: markdown\nbody: body\n",
        )
        .unwrap();
        assert_eq!(source.kind, GIT);
        assert_eq!(source.file_name("hello"), "hello.md");
    }

    #[test]
    fn a_registered_source_has_extra_keys_and_resolves_by_code() {
        let source: Source =
            serde_saphyr::from_str("kind: acme.sheets\npath: content/guests\nsheet: Guests\n")
                .unwrap();
        assert_eq!(source.kind, "acme.sheets");
        assert_eq!(source.options["sheet"], Value::String("Guests".into()));

        struct Sheets;
        #[async_trait]
        impl RecordSource for Sheets {
            async fn pull(&self, _cx: &SourceCx<'_>) -> Result<PullReport, SourceError> {
                Ok(PullReport::default())
            }
            async fn push(&self, _cx: &SourceCx<'_>, _p: &Pushed) -> Result<(), SourceError> {
                Ok(())
            }
            async fn remove(&self, _cx: &SourceCx<'_>, _key: &str) -> Result<(), SourceError> {
                Ok(())
            }
        }
        let registered = vec![RecordSourceReg::new("acme.sheets", Arc::new(Sheets))];
        assert!(resolve("files", &registered).is_some());
        assert!(resolve("git", &registered).is_some());
        assert!(resolve("acme.sheets", &registered).is_some());
        assert!(resolve("acme.other", &registered).is_none());
    }

    #[test]
    fn a_folder_is_read_by_key_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.yaml"), "title: B\n").unwrap();
        std::fs::write(dir.path().join("a.yaml"), "title: A\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let entries = read_dir(dir.path(), Format::Yaml).unwrap();
        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["a", "b"]);
        assert_eq!(entries[0].attrs["title"], Value::String("A".into()));
        assert!(read_dir(&dir.path().join("missing"), Format::Yaml)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_bad_file_name_or_document_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad name.yaml"), "title: X\n").unwrap();
        let err = read_dir(dir.path(), Format::Yaml).unwrap_err().to_string();
        assert!(err.contains("bad name.yaml"), "{err}");
        std::fs::remove_file(dir.path().join("bad name.yaml")).unwrap();
        std::fs::write(dir.path().join("broken.yaml"), "- not\n- a map\n").unwrap();
        let err = read_dir(dir.path(), Format::Yaml).unwrap_err().to_string();
        assert!(err.contains("broken.yaml"), "{err}");
    }
}
