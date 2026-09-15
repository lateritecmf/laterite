//! The media record: a file's stable identity, pointing at a blob.
//!
//! The record is what everything else references, and it outlives any particular
//! bytes. Replacing a file writes a new blob with a new hash, and the record's id
//! and every link to it stay exactly where they were. That is the whole reason a
//! hash is not used as the identity: a hash is an excellent name for bytes and a
//! poor name for a thing.

use chrono::{SecondsFormat, Utc};
use laterite_core::strata::*;
use laterite_core::Db;

/// The media table's columns.
#[derive(Iden)]
pub enum Media {
    Table,
    Id,
    /// The named disk the blob lives on. Stored per record, so a file's physical
    /// location is knowable without consulting configuration that may have moved
    /// on since it was written.
    Disk,
    /// BLAKE3 hex of the bytes. Indexed, and the blob path derives from it.
    Hash,
    Size,
    /// Sniffed at ingest, never taken from the sender.
    ContentType,
    /// Display metadata. Never a path: the storage path is always the hash, so a
    /// hostile filename cannot reach the filesystem.
    OriginalName,
    Title,
    Alt,
    /// Whether this file appears in the media library. A file stored for a
    /// purpose of its own (an import payload, say) is stored but not browsed.
    InLibrary,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

/// A stored file, as the database holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRecord {
    pub id: i64,
    pub disk: String,
    pub hash: String,
    pub size: i64,
    pub content_type: String,
    pub original_name: String,
    pub title: Option<String>,
    pub alt: Option<String>,
    pub in_library: bool,
    pub created_by: Option<i64>,
}

/// What a caller supplies to record an ingested blob.
#[derive(Debug, Clone)]
pub struct NewMedia {
    pub disk: String,
    pub hash: String,
    pub size: i64,
    pub content_type: String,
    /// The sender's filename, kept for display only.
    pub original_name: String,
    pub in_library: bool,
    pub created_by: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Core(#[from] CoreError),
}

/// Inserts a media record and returns its id.
///
/// Two records may share a hash: the same bytes uploaded twice are one blob and
/// two files, each with its own name, title and links. That is the intended
/// outcome, not a duplicate to be collapsed.
pub async fn create(db: &Db, new: NewMedia) -> Result<i64, RecordError> {
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    let stmt = Query::insert()
        .into_table(Media::Table)
        .columns([
            Media::Disk,
            Media::Hash,
            Media::Size,
            Media::ContentType,
            Media::OriginalName,
            Media::InLibrary,
            Media::CreatedBy,
            Media::CreatedAt,
            Media::UpdatedAt,
        ])
        .values_panic([
            new.disk.into(),
            new.hash.into(),
            new.size.into(),
            new.content_type.into(),
            new.original_name.into(),
            new.in_library.into(),
            new.created_by.into(),
            now.clone().into(),
            now.into(),
        ])
        .to_owned();
    Ok(insert_returning_id(db, stmt, Media::Id).await?)
}

/// One record by id, or `None` if it is gone.
pub async fn find(db: &Db, id: i64) -> Result<Option<MediaRecord>, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                Media::Id,
                Media::Disk,
                Media::Hash,
                Media::Size,
                Media::ContentType,
                Media::OriginalName,
                Media::Title,
                Media::Alt,
                Media::InLibrary,
                Media::CreatedBy,
            ])
            .from(Media::Table)
            .and_where(Expr::col(Media::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.map(|r| {
        Ok(MediaRecord {
            id: r.get_int("id")?,
            disk: r.get_text("disk")?,
            hash: r.get_text("hash")?,
            size: r.get_int("size")?,
            content_type: r.get_text("content_type")?,
            original_name: r.get_text("original_name")?,
            title: r.get_text_opt("title")?,
            alt: r.get_text_opt("alt")?,
            in_library: r.get_bool("in_library")?,
            created_by: r.get_int_opt("created_by")?,
        })
    })
    .transpose()
}

/// Deletes a record, returning whether its blob is now unreferenced.
///
/// The blob itself is not touched here: a hash may back several records, so
/// removing bytes is a separate decision from removing a file. `true` means the
/// caller may collect the blob.
pub async fn delete(db: &Db, id: i64) -> Result<bool, RecordError> {
    let Some(record) = find(db, id).await? else {
        return Ok(false);
    };
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(Media::Table)
            .and_where(Expr::col(Media::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;

    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr(Expr::col(Media::Id).count())
            .from(Media::Table)
            .and_where(Expr::col(Media::Hash).eq(record.hash))
            .and_where(Expr::col(Media::Disk).eq(record.disk))
            .to_owned(),
    );
    let remaining: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&sql), values)
        .fetch_one(&db.pool)
        .await?
        .0;
    Ok(remaining == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterite_core::testing::connect_test;

    /// The guard has to outlive the test: dropping it tears the database down.
    async fn db() -> (Db, laterite_core::testing::TestGuard) {
        connect_test(&[crate::migrations::migrations()]).await
    }

    fn new(hash: &str, name: &str) -> NewMedia {
        NewMedia {
            disk: "private".to_string(),
            hash: hash.to_string(),
            size: 12,
            content_type: "image/png".to_string(),
            original_name: name.to_string(),
            in_library: false,
            created_by: None,
        }
    }

    #[tokio::test]
    async fn a_record_keeps_what_the_blob_cannot() {
        let (db, _guard) = db().await;
        let id = create(&db, new(&"a".repeat(64), "logo.png")).await.unwrap();
        let found = find(&db, id).await.unwrap().unwrap();

        assert_eq!(found.hash, "a".repeat(64));
        assert_eq!(found.disk, "private");
        // The filename is display metadata: it never reaches the filesystem,
        // because the storage path is the hash.
        assert_eq!(found.original_name, "logo.png");
        assert!(!found.in_library);
    }

    /// The same bytes uploaded twice are one blob and two files, each with its
    /// own name and its own links. That is the intended outcome of a stable id
    /// pointing at a content-addressed blob, not a duplicate to collapse.
    #[tokio::test]
    async fn one_blob_can_back_several_records() {
        let (db, _guard) = db().await;
        let hash = "b".repeat(64);
        let first = create(&db, new(&hash, "invoice.pdf")).await.unwrap();
        let second = create(&db, new(&hash, "receipt.pdf")).await.unwrap();

        assert_ne!(first, second);
        assert_eq!(find(&db, first).await.unwrap().unwrap().hash, hash);
        assert_eq!(
            find(&db, second).await.unwrap().unwrap().original_name,
            "receipt.pdf"
        );
    }

    /// Removing a file is not removing its bytes. Only the last record holding a
    /// hash releases the blob for collection.
    #[tokio::test]
    async fn only_the_last_record_releases_the_blob() {
        let (db, _guard) = db().await;
        let hash = "c".repeat(64);
        let first = create(&db, new(&hash, "one")).await.unwrap();
        let second = create(&db, new(&hash, "two")).await.unwrap();

        assert!(
            !delete(&db, first).await.unwrap(),
            "another record still holds it"
        );
        assert!(
            delete(&db, second).await.unwrap(),
            "the last one releases it"
        );
    }

    /// Dedup is scoped to a disk: identical bytes on a local disk and in a bucket
    /// are separate physical copies, so deleting one must not free the other.
    #[tokio::test]
    async fn a_blob_on_another_disk_is_a_different_blob() {
        let (db, _guard) = db().await;
        let hash = "d".repeat(64);
        let private = create(&db, new(&hash, "here")).await.unwrap();
        let public = create(
            &db,
            NewMedia {
                disk: "public".to_string(),
                ..new(&hash, "there")
            },
        )
        .await
        .unwrap();

        assert!(
            delete(&db, private).await.unwrap(),
            "its own disk had no other holder"
        );
        assert!(find(&db, public).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn deleting_a_record_that_is_gone_says_so() {
        let (db, _guard) = db().await;
        assert!(!delete(&db, 4242).await.unwrap());
        assert!(find(&db, 4242).await.unwrap().is_none());
    }
}
