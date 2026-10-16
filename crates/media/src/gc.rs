//! Garbage collection: a record outside the library that nothing links to,
//! past a grace period, and a blob no record names any more. Liveness is a
//! query, never a counter.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use laterite_core::strata::*;
use laterite_core::Db;
use std::collections::HashMap;
use std::sync::Arc;

use crate::ingest::blob_path;
use crate::link::MediaLinks;
use crate::record::{self, from_row, Media, MediaRecord, RecordError, COLUMNS};
use crate::StorageDriver;

/// What a collection did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GcReport {
    /// Records removed.
    pub records: usize,
    /// Blobs removed, as the last record naming them went.
    pub blobs: usize,
    /// Blobs left in place as `disk:hash`: the disk was not given, or it
    /// refused the delete.
    pub kept: Vec<String>,
}

/// The records that are garbage: outside the library, linked to nothing, and
/// created before `before`.
pub async fn garbage(db: &Db, before: DateTime<Utc>) -> Result<Vec<MediaRecord>, RecordError> {
    let cutoff = before.to_rfc3339_opts(SecondsFormat::Micros, true);
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(Media::Table)
            .and_where(Expr::col(Media::InLibrary).eq(0))
            .and_where(Expr::col(Media::CreatedAt).lt(cutoff))
            .and_where(
                Expr::col(Media::Id).not_in_subquery(
                    Query::select()
                        .column(MediaLinks::MediaId)
                        .from(MediaLinks::Table)
                        .to_owned(),
                ),
            )
            .order_by(Media::Id, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows.iter().map(from_row).collect::<Result<_, _>>()?)
}

/// Removes every garbage record older than `grace`, and each blob whose last
/// record went, from the disks given by name. A record on a disk not given is
/// still removed; its blob is reported as kept.
pub async fn collect(
    db: &Db,
    disks: &HashMap<String, Arc<dyn StorageDriver>>,
    grace: Duration,
) -> Result<GcReport, RecordError> {
    let before = Utc::now() - grace;
    let mut report = GcReport::default();
    for r in garbage(db, before).await? {
        let variants = crate::variant::list_for(db, r.id).await?;
        crate::variant::delete_for(db, r.id).await?;
        let last = record::delete(db, r.id).await?;
        report.records += 1;
        if !last {
            continue;
        }
        let removed = match disks.get(&r.disk) {
            Some(driver) => {
                for v in &variants {
                    let _ = driver.delete(&v.path).await;
                }
                driver.delete(&blob_path(&r.hash)).await.is_ok()
            }
            None => false,
        };
        if removed {
            report.blobs += 1;
        } else {
            report.kept.push(format!("{}:{}", r.disk, r.hash));
        }
    }
    Ok(report)
}
