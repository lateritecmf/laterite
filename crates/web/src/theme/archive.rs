//! A theme as a zip: `theme.yaml`, `settings.json` when the theme has values,
//! and one file per template under its kind's folder (`layouts/default.htm`,
//! `pages/home.htm`, `partials/nav.htm`, `content/intro.md`,
//! `assets/css/site.css`). A template with settings carries them as YAML
//! above a `==` line, the markup below it; one without is the markup alone.
//!
//! The rows stay the source of truth: an export is for version control and
//! for moving a theme between installs, and an import writes rows through
//! [`store::save_template`], so every replaced template keeps a revision.

use super::{engine, store, Kind, Theme, ThemeConfig, ThemeError};
use laterite_core::Db;
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;

/// The line between a template file's settings and its markup.
const SEPARATOR: &str = "==";
/// The most an archive may hold once read, so a hostile zip cannot exhaust memory.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// What an import did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// Whether the theme was created by this import.
    pub created: bool,
    /// Templates written, new or replaced.
    pub imported: usize,
    /// Templates left as they were, because they existed and `replace` was off.
    pub skipped: usize,
    /// Files the archive held that are not templates, by name.
    pub ignored: Vec<String>,
}

/// A template as its file holds it.
pub fn file_body(settings: &serde_json::Value, markup: &str) -> String {
    match settings.as_object() {
        Some(map) if !map.is_empty() => {
            let yaml = serde_saphyr::to_string(settings).unwrap_or_default();
            format!("{}\n{SEPARATOR}\n{markup}", yaml.trim_end())
        }
        _ => markup.to_string(),
    }
}

/// A file's settings and markup, split at the first `==` line when there is
/// one and the part above it is YAML.
pub fn parse_body(body: &str) -> Result<(serde_json::Value, String), ThemeError> {
    let body = body.replace("\r\n", "\n");
    let Some(at) = body
        .split_inclusive('\n')
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len();
            Some((start, line))
        })
        .find(|(_, line)| line.trim_end() == SEPARATOR)
        .map(|(start, _)| start)
    else {
        return Ok((serde_json::json!({}), body));
    };
    let settings: serde_json::Value = serde_saphyr::from_str(&body[..at])
        .map_err(|e| ThemeError::Config(format!("the settings above `==` do not parse: {e}")))?;
    if !settings.is_object() && !settings.is_null() {
        return Err(ThemeError::Config(
            "the settings above `==` must be a mapping".into(),
        ));
    }
    let settings = if settings.is_null() {
        serde_json::json!({})
    } else {
        settings
    };
    let rest = &body[at..];
    let markup = rest.split_once('\n').map(|(_, m)| m).unwrap_or("");
    Ok((settings, markup.to_string()))
}

/// The theme `code` as a zip.
pub async fn export(db: &Db, code: &str) -> Result<Vec<u8>, ThemeError> {
    let theme = store::find_theme(db, code)
        .await?
        .ok_or_else(|| ThemeError::NotFound(code.to_string()))?;
    let templates = store::list_templates(db, theme.id, None).await?;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("theme.yaml", options)?;
    zip.write_all(theme.config_source.as_bytes())?;
    if theme
        .settings
        .as_object()
        .is_some_and(|map| !map.is_empty())
    {
        zip.start_file("settings.json", options)?;
        zip.write_all(
            serde_json::to_string_pretty(&theme.settings)
                .unwrap_or_default()
                .as_bytes(),
        )?;
    }
    for t in templates {
        zip.start_file(engine::template_name(t.kind, &t.path), options)?;
        zip.write_all(file_body(&t.settings, &t.markup).as_bytes())?;
    }
    Ok(zip.finish()?.into_inner())
}

/// The kind a folder holds.
fn kind_of(folder: &str) -> Option<Kind> {
    Kind::ALL
        .into_iter()
        .find(|k| engine::template_name(*k, "") == format!("{folder}/"))
}

/// Imports `bytes` into the theme `code`, creating it from the archive's
/// `theme.yaml` when it does not exist. An existing template is replaced only
/// with `replace`, each replacement keeping a revision for `saved_by`; the
/// config is replaced on the same condition. A locked theme refuses.
pub async fn import(
    db: &Db,
    code: &str,
    bytes: &[u8],
    replace: bool,
    saved_by: Option<i64>,
) -> Result<ImportReport, ThemeError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut ignored = Vec::new();
    let mut total = 0usize;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let Some(path) = file.enclosed_name() else {
            ignored.push(file.name()?.to_string());
            continue;
        };
        let name = path.to_string_lossy().replace('\\', "/");
        let mut body = String::new();
        total += file.size() as usize;
        if total > MAX_BYTES {
            return Err(ThemeError::Config("the archive is too large".into()));
        }
        file.read_to_string(&mut body)
            .map_err(|_| ThemeError::Config(format!("{name} is not UTF-8 text")))?;
        entries.push((name, body));
    }
    let mut report = import_entries(db, code, entries, replace, saved_by).await?;
    report.ignored.extend(ignored);
    Ok(report)
}

/// Imports the theme folder `dir` into the theme `code`, as [`import`] does a
/// zip: `theme.yaml`, `settings.json` and one file per template under its
/// kind's folder, read relative to `dir`. A hidden entry (a name starting
/// with `.`) is left alone.
pub async fn import_dir(
    db: &Db,
    code: &str,
    dir: &std::path::Path,
    replace: bool,
    saved_by: Option<i64>,
) -> Result<ImportReport, ThemeError> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut total = 0usize;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let read = std::fs::read_dir(&folder)
            .map_err(|e| ThemeError::Config(format!("could not read {}: {e}", folder.display())))?;
        for entry in read {
            let entry = entry.map_err(|e| ThemeError::Config(e.to_string()))?;
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(&path)
                .map_err(|e| ThemeError::Config(format!("could not read {name}: {e}")))?;
            total += bytes.len();
            if total > MAX_BYTES {
                return Err(ThemeError::Config("the theme folder is too large".into()));
            }
            let body = String::from_utf8(bytes)
                .map_err(|_| ThemeError::Config(format!("{name} is not UTF-8 text")))?;
            entries.push((name, body));
        }
    }
    entries.sort();
    import_entries(db, code, entries, replace, saved_by).await
}

/// The import proper, over `(name, body)` entries however they were read.
async fn import_entries(
    db: &Db,
    code: &str,
    entries: Vec<(String, String)>,
    replace: bool,
    saved_by: Option<i64>,
) -> Result<ImportReport, ThemeError> {
    let mut config: Option<String> = None;
    let mut settings: Option<serde_json::Value> = None;
    let mut files: Vec<(Kind, String, String)> = Vec::new();
    let mut report = ImportReport::default();
    for (name, body) in entries {
        match name.as_str() {
            "theme.yaml" => config = Some(body),
            "settings.json" => {
                settings = Some(serde_json::from_str(&body).map_err(|e| {
                    ThemeError::Config(format!("settings.json does not parse: {e}"))
                })?)
            }
            _ => match name.split_once('/') {
                Some((folder, rest)) if !rest.is_empty() => match kind_of(folder) {
                    Some(kind) => files.push((kind, rest.to_string(), body)),
                    None => report.ignored.push(name),
                },
                _ => report.ignored.push(name),
            },
        }
    }
    let theme: Theme = match store::find_theme(db, code).await? {
        Some(theme) => {
            if theme.locked {
                return Err(ThemeError::Locked(code.to_string()));
            }
            match (&config, replace) {
                (Some(yaml), true) => store::update_theme_config(db, code, yaml).await?,
                _ => theme,
            }
        }
        None => {
            let yaml = config.clone().ok_or_else(|| {
                ThemeError::Config("the archive has no theme.yaml to create the theme from".into())
            })?;
            ThemeConfig::parse(&yaml)?;
            report.created = true;
            store::create_theme(db, code, &yaml).await?
        }
    };
    for (kind, path, body) in files {
        if !super::valid_path(&path) {
            return Err(ThemeError::Path(path));
        }
        let exists = store::find_template(db, theme.id, kind, &path)
            .await?
            .is_some();
        if exists && !replace {
            report.skipped += 1;
            continue;
        }
        let (settings, markup) = parse_body(&body)?;
        store::save_template(db, theme.id, kind, &path, Some(settings), &markup, saved_by).await?;
        report.imported += 1;
    }
    if let Some(values) = settings {
        if report.created || replace {
            store::save_settings(db, code, &values).await?;
        }
    }
    Ok(report)
}
