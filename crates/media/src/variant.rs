//! Variants: named, cached derivatives of a media record (a `thumb`, a
//! `card`), each a row pointing at its own blob path, made on the first
//! request behind a lock per file and preset so a burst renders once.

use chrono::{SecondsFormat, Utc};
use futures::TryStreamExt;
use laterite_core::strata::*;
use laterite_core::Db;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::image::{self, ImageError};
use crate::ingest::blob_path;
use crate::preset::Preset;
use crate::record::{MediaRecord, RecordError};
use crate::{Kind, StorageDriver, StorageError};

#[derive(Iden)]
pub enum MediaVariants {
    Table,
    Id,
    MediaId,
    Preset,
    Path,
    Width,
    Height,
    Size,
    ContentType,
    CreatedAt,
}

/// A variant as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub id: i64,
    pub media_id: i64,
    pub preset: String,
    pub path: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size: i64,
    pub content_type: String,
    pub created_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum VariantError {
    #[error(transparent)]
    Record(#[from] RecordError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Image(#[from] ImageError),
    #[error("the file is not an image")]
    NotImage,
    #[error("the blob could not be read: {0}")]
    Read(String),
}

const COLUMNS: [MediaVariants; 9] = [
    MediaVariants::Id,
    MediaVariants::MediaId,
    MediaVariants::Preset,
    MediaVariants::Path,
    MediaVariants::Width,
    MediaVariants::Height,
    MediaVariants::Size,
    MediaVariants::ContentType,
    MediaVariants::CreatedAt,
];

fn from_row(r: &sqlx::any::AnyRow) -> Result<Variant, sqlx::Error> {
    Ok(Variant {
        id: r.get_int("id")?,
        media_id: r.get_int("media_id")?,
        preset: r.get_text("preset")?,
        path: r.get_text("path")?,
        width: r.get_int_opt("width")?,
        height: r.get_int_opt("height")?,
        size: r.get_int("size")?,
        content_type: r.get_text("content_type")?,
        created_at: r.get_text("created_at")?,
    })
}

/// Where a variant's blob lives: beside the originals, under the hash.
pub fn variant_path(hash: &str, preset: &str, extension: &str) -> String {
    format!(
        "variants/{}/{}/{hash}/{preset}.{extension}",
        &hash[0..2],
        &hash[2..4]
    )
}

/// The variant of `media_id` in `preset`, if it was made.
pub async fn find(db: &Db, media_id: i64, preset: &str) -> Result<Option<Variant>, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(MediaVariants::Table)
            .and_where(Expr::col(MediaVariants::MediaId).eq(media_id))
            .and_where(Expr::col(MediaVariants::Preset).eq(preset))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

/// Every variant made of `media_id`.
pub async fn list_for(db: &Db, media_id: i64) -> Result<Vec<Variant>, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(MediaVariants::Table)
            .and_where(Expr::col(MediaVariants::MediaId).eq(media_id))
            .order_by(MediaVariants::Id, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows.iter().map(from_row).collect::<Result<_, _>>()?)
}

/// Forgets every variant of `media_id`; the blobs are the caller's to remove.
pub async fn delete_for(db: &Db, media_id: i64) -> Result<u64, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(MediaVariants::Table)
            .and_where(Expr::col(MediaVariants::MediaId).eq(media_id))
            .to_owned(),
    );
    Ok(bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?
        .rows_affected())
}

async fn insert(
    db: &Db,
    media_id: i64,
    preset: &str,
    path: &str,
    rendered: &image::Rendered,
) -> Result<Variant, RecordError> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    let stmt = Query::insert()
        .into_table(MediaVariants::Table)
        .columns([
            MediaVariants::MediaId,
            MediaVariants::Preset,
            MediaVariants::Path,
            MediaVariants::Width,
            MediaVariants::Height,
            MediaVariants::Size,
            MediaVariants::ContentType,
            MediaVariants::CreatedAt,
        ])
        .values_panic([
            media_id.into(),
            preset.into(),
            path.into(),
            i64::from(rendered.width).into(),
            i64::from(rendered.height).into(),
            (rendered.bytes.len() as i64).into(),
            rendered.content_type.into(),
            now.clone().into(),
        ])
        .to_owned();
    let id = insert_returning_id(db, stmt, MediaVariants::Id).await?;
    Ok(Variant {
        id,
        media_id,
        preset: preset.to_string(),
        path: path.to_string(),
        width: Some(i64::from(rendered.width)),
        height: Some(i64::from(rendered.height)),
        size: rendered.bytes.len() as i64,
        content_type: rendered.content_type.to_string(),
        created_at: now,
    })
}

/// The maker of variants: holds one lock per file and preset, so a burst of
/// requests for a variant that does not exist yet renders it once.
#[derive(Default)]
pub struct Variants {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Variants {
    pub fn new() -> Self {
        Self::default()
    }

    /// The variant of `record` in `preset`, made and stored on `disk` when it
    /// does not exist yet.
    pub async fn ensure(
        &self,
        db: &Db,
        disk: &dyn StorageDriver,
        record: &MediaRecord,
        preset: &Preset,
    ) -> Result<Variant, VariantError> {
        if record.kind() != Kind::Image || record.content_type == "image/svg+xml" {
            return Err(VariantError::NotImage);
        }
        if let Some(v) = find(db, record.id, &preset.name).await? {
            return Ok(v);
        }
        let key = format!("{}:{}:{}", record.disk, record.hash, preset.name);
        let lock = {
            let mut locks = self.locks.lock().await;
            Arc::clone(locks.entry(key.clone()).or_default())
        };
        let result = {
            let _held = lock.lock().await;
            match find(db, record.id, &preset.name).await? {
                Some(v) => Ok(v),
                None => make(db, disk, record, preset).await,
            }
        };
        let mut locks = self.locks.lock().await;
        if Arc::strong_count(&lock) <= 2 {
            locks.remove(&key);
        }
        result
    }
}

async fn make(
    db: &Db,
    disk: &dyn StorageDriver,
    record: &MediaRecord,
    preset: &Preset,
) -> Result<Variant, VariantError> {
    let mut stream = disk.get(&blob_path(&record.hash)).await?;
    let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(record.size).unwrap_or(0));
    while let Some(chunk) = stream
        .try_next()
        .await
        .map_err(|e| VariantError::Read(e.to_string()))?
    {
        bytes.extend_from_slice(&chunk);
    }
    let orientation = record
        .meta
        .get("orientation")
        .and_then(|o| o.as_u64())
        .and_then(|o| u8::try_from(o).ok())
        .unwrap_or(1);
    let preset_owned = preset.clone();
    let rendered =
        tokio::task::spawn_blocking(move || image::render(&bytes, &preset_owned, orientation))
            .await
            .map_err(|e| VariantError::Read(e.to_string()))??;
    let path = variant_path(&record.hash, &preset.name, rendered.extension);
    disk.put(&path, crate::stream_of(rendered.bytes.clone()))
        .await?;
    Ok(insert(db, record.id, &preset.name, &path, &rendered).await?)
}
