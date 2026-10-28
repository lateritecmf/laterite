//! A theme read from a folder on disk into a private in-memory database, so a
//! site is built or previewed from its files alone, with no application and
//! nothing configured. The folder is the theme's export layout; its name is
//! the theme's code.

use super::{archive, ThemeModule};
use anyhow::Context;
use laterite_core::config::DatabaseConfig;
use laterite_core::Db;
use std::path::Path;

/// A theme folder loaded into memory.
pub struct Loaded {
    /// The database holding the theme, private to this load.
    pub db: Db,
    /// The theme's code: the folder's name.
    pub code: String,
    /// What the load wrote.
    pub report: archive::ImportReport,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("code", &self.code)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// Loads the theme folder `dir` into a fresh in-memory database, with the
/// framework's and the theme module's tables, and returns it ready to build.
pub async fn load(dir: &Path) -> anyhow::Result<Loaded> {
    let code = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .with_context(|| {
            format!(
                "{} has no folder name to use as the theme code",
                dir.display()
            )
        })?;
    if !dir.is_dir() {
        anyhow::bail!("{} is not a theme folder", dir.display());
    }
    if !dir.join("theme.yaml").is_file() {
        anyhow::bail!("{} has no theme.yaml", dir.display());
    }
    let db = laterite_core::db::connect(&DatabaseConfig {
        url: "sqlite::memory:".to_string(),
        // One connection: a second one would be another empty database.
        max_connections: 1,
        acquire_timeout_secs: 5,
    })
    .await
    .context("could not open an in-memory database")?;
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(laterite_core::Module::migrations(&ThemeModule));
    laterite_core::migration::run(&db.pool, db.backend, &sets)
        .await
        .context("could not prepare the in-memory database")?;
    let report = archive::import_dir(&db, &code, dir, true, None)
        .await
        .with_context(|| format!("could not load the theme folder {}", dir.display()))?;
    Ok(Loaded { db, code, report })
}
