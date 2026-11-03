//! Where a theme lives: a driver around the database the engine renders from.
//!
//! The engine, the build and the preview read theme rows. A source keeps
//! those rows in step with wherever the theme is kept: `pull` brings it in
//! (at boot and before a build), `push` writes one change back (after every
//! save in the admin), `publish` makes the pushed changes visible to a host
//! when the source has such a step. `database` is the rows themselves,
//! `folder` is the theme folder, `git` is the folder committed and pushed; a
//! module registers any other source as a [`ThemeSourceReg`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use laterite_core::config::ThemeConfig;
use laterite_core::strata::async_trait;
use laterite_core::{Db, Registry};

use super::{archive, engine, folder, store, Kind, ThemeError};

/// What a source is given to work with.
pub struct SourceCx<'a> {
    /// The database the engine renders from.
    pub db: &'a Db,
    /// The project root, the directory holding `config/`.
    pub root: &'a Path,
    /// The `[theme]` section.
    pub config: &'a ThemeConfig,
}

impl SourceCx<'_> {
    /// The theme folder, absolute, when the config names one.
    pub fn folder(&self) -> Result<PathBuf, ThemeError> {
        self.config
            .folder
            .as_deref()
            .map(|f| self.root.join(f))
            .ok_or_else(|| ThemeError::Config("theme.folder is not set".into()))
    }

    /// The theme code a folder source keeps: the folder's name.
    pub fn folder_code(&self) -> Result<String, ThemeError> {
        let dir = self.folder()?;
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| ThemeError::Config(format!("{} has no folder name", dir.display())))
    }
}

/// One change the admin made, for a source to write back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A template was saved (or restored from a revision).
    Template {
        theme: String,
        kind: Kind,
        path: String,
    },
    /// A template was deleted.
    TemplateRemoved {
        theme: String,
        kind: Kind,
        path: String,
    },
    /// The theme's config was saved.
    Config { theme: String },
    /// The theme's settings values were saved.
    Settings { theme: String },
    /// The whole theme changed (an import, a creation): everything is written.
    Theme { theme: String },
}

impl Change {
    /// The theme the change belongs to.
    pub fn theme(&self) -> &str {
        match self {
            Change::Template { theme, .. }
            | Change::TemplateRemoved { theme, .. }
            | Change::Config { theme }
            | Change::Settings { theme }
            | Change::Theme { theme } => theme,
        }
    }
}

/// What a publish did, for the screen or the command to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub note: String,
}

/// A theme source.
#[async_trait]
pub trait ThemeSource: Send + Sync {
    /// Brings the theme into the database.
    async fn pull(&self, cx: &SourceCx<'_>) -> Result<(), ThemeError>;
    /// Writes one change back.
    async fn push(&self, cx: &SourceCx<'_>, change: &Change) -> Result<(), ThemeError>;
    /// Makes the pushed changes visible to the host. Default: nothing to do.
    async fn publish(&self, cx: &SourceCx<'_>) -> Result<Published, ThemeError> {
        let _ = cx;
        Ok(Published {
            note: "nothing to publish: the source has no publish step".into(),
        })
    }
}

/// A source a module contributes, chosen by `[theme] source = "<code>"`.
pub struct ThemeSourceReg {
    pub code: String,
    pub source: Arc<dyn ThemeSource>,
}

impl ThemeSourceReg {
    pub fn new(code: impl Into<String>, source: Arc<dyn ThemeSource>) -> Self {
        Self {
            code: code.into(),
            source,
        }
    }
}

/// The project a source works in: contributed at boot by the application,
/// read by the theme handle and the theme module.
pub use laterite_admin::statics::SiteProject;

/// The source `config.source` names, among those registered.
pub fn resolve(
    registry: &Registry,
    config: &ThemeConfig,
) -> Result<Arc<dyn ThemeSource>, ThemeError> {
    let code = config.effective_source();
    registry
        .contributions::<ThemeSourceReg>()
        .into_iter()
        .find(|c| c.item.code == code)
        .map(|c| Arc::clone(&c.item.source))
        .ok_or_else(|| {
            ThemeError::Config(format!(
                "theme.source {code:?} is not registered: the built-in sources are database and \
                 folder, git with the theme-git feature"
            ))
        })
}

/// The main shape: the theme is the rows. Nothing to pull or push.
pub struct Database;

#[async_trait]
impl ThemeSource for Database {
    async fn pull(&self, _cx: &SourceCx<'_>) -> Result<(), ThemeError> {
        Ok(())
    }
    async fn push(&self, _cx: &SourceCx<'_>, _change: &Change) -> Result<(), ThemeError> {
        Ok(())
    }
}

/// The theme folder: read into the database on pull, written file by file
/// on push. Only the folder's own theme (the one its name codes) is kept;
/// a change to another theme is left in the rows.
pub struct Folder;

impl Folder {
    /// Writes every file of the theme into the folder.
    async fn write_all(cx: &SourceCx<'_>, code: &str) -> Result<(), ThemeError> {
        let dir = cx.folder()?;
        let theme = store::find_theme(cx.db, code)
            .await?
            .ok_or_else(|| ThemeError::NotFound(code.to_string()))?;
        write_file(&dir.join("theme.yaml"), &theme.config_source)?;
        write_settings(&dir, &theme.settings)?;
        for t in store::list_templates(cx.db, theme.id, None).await? {
            write_file(
                &dir.join(engine::template_name(t.kind, &t.path)),
                &archive::file_body(&t.settings, &t.markup),
            )?;
        }
        Ok(())
    }
}

fn write_file(path: &Path, body: &str) -> Result<(), ThemeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ThemeError::Config(format!("could not create {}: {e}", parent.display()))
        })?;
    }
    std::fs::write(path, body)
        .map_err(|e| ThemeError::Config(format!("could not write {}: {e}", path.display())))
}

fn write_settings(dir: &Path, settings: &serde_json::Value) -> Result<(), ThemeError> {
    let path = dir.join("settings.json");
    if settings.as_object().is_some_and(|m| !m.is_empty()) {
        write_file(
            &path,
            &serde_json::to_string_pretty(settings).unwrap_or_default(),
        )
    } else if path.is_file() {
        std::fs::remove_file(&path)
            .map_err(|e| ThemeError::Config(format!("could not remove {}: {e}", path.display())))
    } else {
        Ok(())
    }
}

#[async_trait]
impl ThemeSource for Folder {
    async fn pull(&self, cx: &SourceCx<'_>) -> Result<(), ThemeError> {
        let dir = cx.folder()?;
        folder::load_into(cx.db, &dir)
            .await
            .map(|_| ())
            .map_err(|e| ThemeError::Config(format!("{e:#}")))
    }

    async fn push(&self, cx: &SourceCx<'_>, change: &Change) -> Result<(), ThemeError> {
        if change.theme() != cx.folder_code()? {
            return Ok(());
        }
        let dir = cx.folder()?;
        match change {
            Change::Template { theme, kind, path } => {
                let row = store::find_theme(cx.db, theme)
                    .await?
                    .ok_or_else(|| ThemeError::NotFound(theme.clone()))?;
                let template = store::find_template(cx.db, row.id, *kind, path)
                    .await?
                    .ok_or_else(|| ThemeError::TemplateNotFound(path.clone()))?;
                write_file(
                    &dir.join(engine::template_name(*kind, path)),
                    &archive::file_body(&template.settings, &template.markup),
                )
            }
            Change::TemplateRemoved { kind, path, .. } => {
                let file = dir.join(engine::template_name(*kind, path));
                if file.is_file() {
                    std::fs::remove_file(&file).map_err(|e| {
                        ThemeError::Config(format!("could not remove {}: {e}", file.display()))
                    })?;
                }
                Ok(())
            }
            Change::Config { theme } => {
                let row = store::find_theme(cx.db, theme)
                    .await?
                    .ok_or_else(|| ThemeError::NotFound(theme.clone()))?;
                write_file(&dir.join("theme.yaml"), &row.config_source)
            }
            Change::Settings { theme } => {
                let row = store::find_theme(cx.db, theme)
                    .await?
                    .ok_or_else(|| ThemeError::NotFound(theme.clone()))?;
                write_settings(&dir, &row.settings)
            }
            Change::Theme { theme } => Self::write_all(cx, theme).await,
        }
    }

    async fn publish(&self, _cx: &SourceCx<'_>) -> Result<Published, ThemeError> {
        Ok(Published {
            note: "the folder is written; commit it to publish".into(),
        })
    }
}

/// The folder, committed on every push and pushed to the remote on publish.
/// Runs the `git` command in the project root.
#[cfg(feature = "theme-git")]
pub struct Git;

#[cfg(feature = "theme-git")]
impl Git {
    fn git(root: &Path, args: &[&str]) -> Result<String, ThemeError> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|e| ThemeError::Config(format!("could not run git: {e}")))?;
        if !out.status.success() {
            return Err(ThemeError::Config(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}

#[cfg(feature = "theme-git")]
#[async_trait]
impl ThemeSource for Git {
    async fn pull(&self, cx: &SourceCx<'_>) -> Result<(), ThemeError> {
        // A remote that is not there yet is not an error: the first push makes it.
        let _ = Self::git(
            cx.root,
            &[
                "pull",
                "--ff-only",
                &cx.config.git.remote,
                &cx.config.git.branch,
            ],
        );
        Folder.pull(cx).await
    }

    async fn push(&self, cx: &SourceCx<'_>, change: &Change) -> Result<(), ThemeError> {
        if change.theme() != cx.folder_code()? {
            return Ok(());
        }
        Folder.push(cx, change).await?;
        let folder = cx.folder()?;
        let folder = folder.to_string_lossy().to_string();
        Self::git(cx.root, &["add", "-A", "--", &folder])?;
        let staged = Self::git(cx.root, &["diff", "--cached", "--quiet"]).is_err();
        if staged {
            let message = match change {
                Change::Template { kind, path, .. } => {
                    format!("Saved {}", engine::template_name(*kind, path))
                }
                Change::TemplateRemoved { kind, path, .. } => {
                    format!("Removed {}", engine::template_name(*kind, path))
                }
                Change::Config { .. } => "Saved the theme config".to_string(),
                Change::Settings { .. } => "Saved the theme settings".to_string(),
                Change::Theme { theme } => format!("Saved the theme {theme}"),
            };
            Self::git(cx.root, &["commit", "-q", "-m", &message, "--", &folder])?;
        }
        Ok(())
    }

    async fn publish(&self, cx: &SourceCx<'_>) -> Result<Published, ThemeError> {
        Self::git(
            cx.root,
            &["push", &cx.config.git.remote, &cx.config.git.branch],
        )?;
        Ok(Published {
            note: format!(
                "pushed {} to {}",
                cx.config.git.branch, cx.config.git.remote
            ),
        })
    }
}
