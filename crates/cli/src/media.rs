//! `lat media`: garbage collection and verification over the local disks.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use laterite_core::Db;
use laterite_media::{gc, verify, LocalDisk, StorageDriver};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum MediaCommand {
    /// Remove files outside the library that nothing links to, and the blobs
    /// no record names any more.
    Gc(GcArgs),
    /// Re-hash every stored file against its record.
    Verify(VerifyArgs),
}

#[derive(Args)]
pub struct GcArgs {
    /// Leave files younger than this many hours alone, so an upload that is
    /// still being attached is not collected under the author. Default: 24.
    #[arg(long, default_value_t = 24)]
    pub grace: u64,
    /// List what would go without removing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// The storage directory. Default: the application's `backend.storage`.
    #[arg(long)]
    pub storage: Option<String>,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// The storage directory. Default: the application's `backend.storage`.
    #[arg(long)]
    pub storage: Option<String>,
}

pub async fn run(command: MediaCommand, db: Db) -> Result<()> {
    match command {
        MediaCommand::Gc(args) => {
            let disks = local_disks(args.storage);
            let grace = chrono::Duration::hours(i64::try_from(args.grace).unwrap_or(i64::MAX));
            if args.dry_run {
                let doomed = gc::garbage(&db, chrono::Utc::now() - grace)
                    .await
                    .context("could not list the unused files")?;
                for r in &doomed {
                    println!("{}\t{}\t{}\t{}", r.id, r.disk, r.hash, r.original_name);
                }
                println!("{} unused file(s); nothing removed", doomed.len());
                return Ok(());
            }
            let report = gc::collect(&db, &disks, grace)
                .await
                .context("could not collect the unused files")?;
            println!(
                "removed {} record(s) and {} blob(s)",
                report.records, report.blobs
            );
            for kept in &report.kept {
                println!("kept {kept}: disk not reachable from here");
            }
            Ok(())
        }
        MediaCommand::Verify(args) => {
            let disks = local_disks(args.storage);
            let report = verify::run(&db, &disks)
                .await
                .context("could not verify the stored files")?;
            for f in &report.faults {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    f.id, f.disk, f.hash, f.original_name, f.reason
                );
            }
            println!(
                "{} ok, {} faulty, {} on disks not reachable from here",
                report.ok,
                report.faults.len(),
                report.skipped
            );
            if report.faults.is_empty() {
                Ok(())
            } else {
                anyhow::bail!("{} file(s) failed verification", report.faults.len())
            }
        }
    }
}

/// The two disks the framework keeps under the storage directory, by the
/// names the admin registers them under: the library at `media` and the
/// private one at `private`. A disk the application registered itself (an
/// object store) is not reachable from here and is reported as skipped.
fn local_disks(storage: Option<String>) -> HashMap<String, Arc<dyn StorageDriver>> {
    let root = storage.unwrap_or_else(storage_from_project);
    let root = root.trim_end_matches('/');
    HashMap::from([
        (
            laterite_admin::medialib::DISK.to_string(),
            Arc::new(LocalDisk::new(format!("{root}/media"))) as Arc<dyn StorageDriver>,
        ),
        (
            laterite_admin::import::DISK.to_string(),
            Arc::new(LocalDisk::new(format!("{root}/private"))) as Arc<dyn StorageDriver>,
        ),
    ])
}

/// `backend.storage` from the application found upward; `storage` without one.
fn storage_from_project() -> String {
    #[derive(serde::Deserialize, Default)]
    struct Backend {
        #[serde(default)]
        storage: Option<String>,
    }
    #[derive(serde::Deserialize, Default)]
    struct Only {
        #[serde(default)]
        backend: Backend,
    }
    crate::project::Project::locate()
        .and_then(|p| p.load::<Only>())
        .ok()
        .and_then(|o| o.backend.storage)
        .unwrap_or_else(|| "storage".to_string())
}
