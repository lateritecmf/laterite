//! Verification: every record's blob re-hashed against its row, so a lost
//! file, a truncated write or bit-rot shows up as a named fault.

use futures::TryStreamExt;
use laterite_core::strata::*;
use laterite_core::Db;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ingest::blob_path;
use crate::record::{from_row, Media, RecordError, COLUMNS};
use crate::StorageDriver;

/// One record whose blob did not check out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub id: i64,
    pub disk: String,
    pub hash: String,
    pub original_name: String,
    pub reason: String,
}

/// What a verification found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Records whose blob is present and hashes to the row's hash.
    pub ok: usize,
    /// Records on a disk not given, left unchecked.
    pub skipped: usize,
    pub faults: Vec<Fault>,
}

/// Re-hashes every record's blob on the disks given by name.
pub async fn run(
    db: &Db,
    disks: &HashMap<String, Arc<dyn StorageDriver>>,
) -> Result<VerifyReport, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(Media::Table)
            .order_by(Media::Id, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let mut report = VerifyReport::default();
    for row in &rows {
        let r = from_row(row)?;
        let Some(driver) = disks.get(&r.disk) else {
            report.skipped += 1;
            continue;
        };
        match check(driver.as_ref(), &r.hash, r.size).await {
            Ok(()) => report.ok += 1,
            Err(reason) => report.faults.push(Fault {
                id: r.id,
                disk: r.disk,
                hash: r.hash,
                original_name: r.original_name,
                reason,
            }),
        }
    }
    Ok(report)
}

async fn check(driver: &dyn StorageDriver, hash: &str, size: i64) -> Result<(), String> {
    let mut stream = driver
        .get(&blob_path(hash))
        .await
        .map_err(|e| format!("missing: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut read: u64 = 0;
    while let Some(chunk) = stream
        .try_next()
        .await
        .map_err(|e| format!("unreadable: {e}"))?
    {
        read += chunk.len() as u64;
        hasher.update(&chunk);
    }
    let actual = hasher.finalize().to_hex().to_string();
    if actual != hash {
        return Err(format!("hash mismatch: the bytes hash to {actual}"));
    }
    if i64::try_from(read).ok() != Some(size) {
        return Err(format!("size {read}, the row says {size}"));
    }
    Ok(())
}
