//! `lat crypt`: operations under the application key.

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::project::Project;

#[derive(Subcommand)]
pub enum CryptCommand {
    /// Re-seal every registered sealed column under the current app.key, so a
    /// previous key can be retired.
    Rotate,
}

pub async fn run(command: CryptCommand, database_url: Option<String>) -> Result<()> {
    match command {
        CryptCommand::Rotate => rotate(database_url).await,
    }
}

async fn rotate(database_url: Option<String>) -> Result<()> {
    let project = Project::locate()?;
    let config: laterite_admin::AppConfig = project.load()?;
    let db = crate::connect(database_url).await?;
    let legacy = laterite_admin::legacy_key_material(&db, config.backend.secret.as_deref())
        .await
        .context("reading the stored key")?;
    let crypt = laterite_admin::crypt_from_config(&config, &legacy)?;
    let done = laterite_core::crypt::rotate(&db, &crypt)
        .await
        .context("re-sealing")?;
    println!(
        "Re-sealed {} value(s) under key {}; {} already there; {} would not open and were left.",
        done.resealed,
        crypt.current_id(),
        done.current,
        done.unreadable
    );
    if done.unreadable > 0 {
        println!(
            "A value that does not open was sealed under a key no longer in app.previous_keys."
        );
    }
    Ok(())
}
