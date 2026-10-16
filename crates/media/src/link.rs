//! Links: the relation between a media record and the record and field that
//! holds it. Liveness is a query over these rows, never a counter: a record
//! outside the library with no link is garbage, and a library file still linked
//! names who uses it.

use chrono::{SecondsFormat, Utc};
use laterite_core::strata::*;
use laterite_core::Db;

use crate::record::RecordError;

#[derive(Iden)]
pub enum MediaLinks {
    Table,
    Id,
    MediaId,
    OwnerType,
    OwnerId,
    Field,
    SortOrder,
    CreatedAt,
}

/// One link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: i64,
    pub media_id: i64,
    pub owner_type: String,
    pub owner_id: String,
    pub field: String,
    pub sort_order: i64,
}

/// A record, a field and the media it holds, in order.
#[derive(Debug, Clone, Copy)]
pub struct Owner<'a> {
    pub owner_type: &'a str,
    pub owner_id: &'a str,
    pub field: &'a str,
}

fn from_row(r: &sqlx::any::AnyRow) -> Result<Link, sqlx::Error> {
    Ok(Link {
        id: r.get_int("id")?,
        media_id: r.get_int("media_id")?,
        owner_type: r.get_text("owner_type")?,
        owner_id: r.get_text("owner_id")?,
        field: r.get_text("field")?,
        sort_order: r.get_int("sort_order")?,
    })
}

const COLUMNS: [MediaLinks; 6] = [
    MediaLinks::Id,
    MediaLinks::MediaId,
    MediaLinks::OwnerType,
    MediaLinks::OwnerId,
    MediaLinks::Field,
    MediaLinks::SortOrder,
];

/// The media an owner's field holds, in order.
pub async fn of_owner(db: &Db, owner: Owner<'_>) -> Result<Vec<Link>, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(MediaLinks::Table)
            .and_where(Expr::col(MediaLinks::OwnerType).eq(owner.owner_type))
            .and_where(Expr::col(MediaLinks::OwnerId).eq(owner.owner_id))
            .and_where(Expr::col(MediaLinks::Field).eq(owner.field))
            .order_by(MediaLinks::SortOrder, Order::Asc)
            .order_by(MediaLinks::Id, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows.iter().map(from_row).collect::<Result<_, _>>()?)
}

/// Who uses a media record: every link to it.
pub async fn users_of(db: &Db, media_id: i64) -> Result<Vec<Link>, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(MediaLinks::Table)
            .and_where(Expr::col(MediaLinks::MediaId).eq(media_id))
            .order_by(MediaLinks::Id, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows.iter().map(from_row).collect::<Result<_, _>>()?)
}

/// Replaces what an owner's field holds with `media_ids`, in that order: links
/// no longer named are removed, new ones added, the rest reordered.
pub async fn set(db: &Db, owner: Owner<'_>, media_ids: &[i64]) -> Result<(), RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(MediaLinks::Table)
            .and_where(Expr::col(MediaLinks::OwnerType).eq(owner.owner_type))
            .and_where(Expr::col(MediaLinks::OwnerId).eq(owner.owner_id))
            .and_where(Expr::col(MediaLinks::Field).eq(owner.field))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    for (order, media_id) in media_ids.iter().enumerate() {
        let stmt = Query::insert()
            .into_table(MediaLinks::Table)
            .columns([
                MediaLinks::MediaId,
                MediaLinks::OwnerType,
                MediaLinks::OwnerId,
                MediaLinks::Field,
                MediaLinks::SortOrder,
                MediaLinks::CreatedAt,
            ])
            .values_panic([
                (*media_id).into(),
                owner.owner_type.into(),
                owner.owner_id.into(),
                owner.field.into(),
                (order as i64).into(),
                now.clone().into(),
            ])
            .to_owned();
        insert_returning_id(db, stmt, MediaLinks::Id).await?;
    }
    Ok(())
}

/// Removes every link an owner holds, in every field: what a record's deletion calls.
pub async fn clear_owner(db: &Db, owner_type: &str, owner_id: &str) -> Result<u64, RecordError> {
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(MediaLinks::Table)
            .and_where(Expr::col(MediaLinks::OwnerType).eq(owner_type))
            .and_where(Expr::col(MediaLinks::OwnerId).eq(owner_id))
            .to_owned(),
    );
    let done = bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(done.rows_affected())
}
