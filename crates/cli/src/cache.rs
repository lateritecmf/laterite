//! `lat cache`: the shared generations every process reads.

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::project::Project;

#[derive(Subcommand)]
pub enum CacheCommand {
    /// Move every namespace on (or one), so every process reads afresh.
    Clear {
        /// One namespace, such as `records::posts`; every known one when unset.
        #[arg(long)]
        namespace: Option<String>,
    },
    /// Show every namespace the deployment knows, with its generation.
    List,
}

pub async fn run(command: CacheCommand, database_url: Option<String>) -> Result<()> {
    let db = crate::connect(database_url).await?;
    let cache = open(&db).await?;
    match command {
        CacheCommand::Clear { namespace } => match namespace {
            Some(namespace) => {
                cache.allow(namespace.clone());
                let generation = cache
                    .bump(&namespace)
                    .await
                    .with_context(|| format!("bumping {namespace}"))?;
                println!("{namespace} is at generation {generation}.");
            }
            None => {
                let moved = cache.bump_all().await.context("bumping every namespace")?;
                println!("Moved {moved} namespace(s) on; every cached value is read afresh.");
            }
        },
        CacheCommand::List => {
            let known = cache.known().await.context("reading the generations")?;
            if known.is_empty() {
                println!("No namespace has been used yet.");
            }
            for (namespace, generation) in known {
                println!("{namespace}\t{generation}");
            }
        }
    }
    Ok(())
}

/// The deployment's generations: through Redis when `cache.store` is
/// `redis`, else the database table. Values are never kept here.
pub async fn open(db: &laterite_core::Db) -> Result<laterite_core::Cache> {
    let config = match Project::locate() {
        Ok(project) => project
            .load::<laterite_admin::AppConfig>()
            .map(|c| c.cache)
            .unwrap_or_default(),
        Err(_) => laterite_core::config::CacheConfig::default(),
    };
    laterite_core::cache::from_config(&config, db, true)
        .await
        .context("opening the cache")
}
