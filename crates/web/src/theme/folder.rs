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
    let db = laterite_core::db::connect(&DatabaseConfig::memory())
        .await
        .context("could not open an in-memory database")?;
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(laterite_core::Module::migrations(&ThemeModule));
    laterite_core::migration::run(&db.pool, db.backend, &sets)
        .await
        .context("could not prepare the in-memory database")?;
    let (code, report) = load_into(&db, dir).await?;
    Ok(Loaded { db, code, report })
}

/// Loads the theme folder `dir` into `db`, whose theme tables are migrated,
/// replacing the theme of that code if the database already holds one, and
/// returns the code with the import's report.
pub async fn load_into(db: &Db, dir: &Path) -> anyhow::Result<(String, archive::ImportReport)> {
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
    if super::store::find_theme(db, &code).await?.is_some() {
        super::store::delete_theme(db, &code).await?;
    }
    let report = archive::import_dir(db, &code, dir, true, None)
        .await
        .with_context(|| format!("could not load the theme folder {}", dir.display()))?;
    Ok((code, report))
}

/// The renderer a static website's binary builds with: the theme folder
/// through the same build path, with every module's markup and components.
pub struct Renderer;

#[laterite_core::strata::async_trait]
impl laterite_admin::statics::StaticRenderer for Renderer {
    async fn load(
        &self,
        cx: &laterite_admin::statics::StaticBuildCx<'_>,
    ) -> anyhow::Result<String> {
        let (code, _) = load_into(cx.db, cx.folder).await?;
        Ok(code)
    }

    async fn build(
        &self,
        cx: &laterite_admin::statics::StaticBuildCx<'_>,
        code: &str,
    ) -> anyhow::Result<Vec<String>> {
        let markup: Vec<std::sync::Arc<dyn super::markup::Markup>> = cx
            .contributions
            .contributions::<super::markup::MarkupReg>()
            .into_iter()
            .map(|c| std::sync::Arc::clone(&c.item.markup))
            .collect();
        let components: Vec<&super::component::ComponentReg> = cx
            .contributions
            .contributions::<super::component::ComponentReg>()
            .into_iter()
            .map(|c| c.item)
            .collect();
        let report =
            super::build::build_with(cx.db, code, cx.out, cx.base_url, &markup, &components)
                .await
                .with_context(|| format!("could not build the theme {code:?}"))?;
        if !report.skipped.is_empty() {
            println!(
                "Skipped {} page(s) with parameters or hidden: {}",
                report.skipped.len(),
                report.skipped.join(", ")
            );
        }
        Ok(report.pages)
    }
}
