//! Typed models: a struct view of a table, read and written without
//! hand-decoding rows.
//!
//! `#[derive(Model)]` implements [`Model`] for a struct whose fields are its
//! columns; [`find`], [`all`], [`insert`] and [`update`] read and write it, and
//! [`has_many`] and [`find_many`] load related models for a whole page in one
//! query per relation.

use std::collections::HashMap;

pub use chrono::{DateTime, Utc};
use sea_query::{Alias, Expr, Order, Query};
pub use sqlx::any::AnyRow;
/// The error a model read or write returns.
pub use sqlx::Error;

use crate::query::{bind_values, build, insert_returning_id, text_cast, AnyRowExt};
use crate::record::{AttrValue, Record};
use crate::Db;

/// A struct that maps one row of a table. Implement it with `#[derive(Model)]`.
pub trait Model: Sized + Send {
    /// The table the model reads and writes.
    const TABLE: &'static str;
    /// The id column.
    const ID: &'static str = "id";
    /// Every column the model reads, its id first.
    const COLUMNS: &'static [&'static str];

    /// The model from a row holding every column in [`COLUMNS`](Self::COLUMNS).
    fn from_row(row: &AnyRow) -> Result<Self, sqlx::Error>;
    /// The model's columns as a record, its id left out.
    fn to_record(&self) -> Record;
    /// The model's id.
    fn id(&self) -> i64;
}

/// A column value a model field can hold, read from a row.
pub trait FromColumn: Sized {
    /// The value of `column` in `row`.
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error>;
}

impl FromColumn for String {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        Ok(row.get_text_opt(column)?.unwrap_or_default())
    }
}

/// The column's text, trimmed: every model read selects its columns cast to
/// text, so one decode works on every backend.
fn text(row: &AnyRow, column: &str) -> Result<String, sqlx::Error> {
    Ok(row.get_text(column)?.trim().to_string())
}

impl FromColumn for i64 {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        text(row, column)?
            .parse()
            .map_err(|e: std::num::ParseIntError| sqlx::Error::Decode(Box::new(e)))
    }
}

impl FromColumn for i32 {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        text(row, column)?
            .parse()
            .map_err(|e: std::num::ParseIntError| sqlx::Error::Decode(Box::new(e)))
    }
}

impl FromColumn for bool {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        Ok(matches!(
            text(row, column)?.to_ascii_lowercase().as_str(),
            "1" | "true" | "t"
        ))
    }
}

impl FromColumn for f64 {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        text(row, column)?
            .parse()
            .map_err(|e: std::num::ParseFloatError| sqlx::Error::Decode(Box::new(e)))
    }
}

impl FromColumn for DateTime<Utc> {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        let text = text(row, column)?;
        DateTime::parse_from_rfc3339(&text)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))
    }
}

impl FromColumn for serde_json::Value {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        match row.get_text_opt(column)? {
            Some(text) if !text.is_empty() => {
                serde_json::from_str(&text).map_err(|e| sqlx::Error::Decode(Box::new(e)))
            }
            _ => Ok(serde_json::Value::Null),
        }
    }
}

impl<T: FromColumn> FromColumn for Option<T> {
    fn from_column(row: &AnyRow, column: &str) -> Result<Self, sqlx::Error> {
        match row.get_text_opt(column)? {
            None => Ok(None),
            Some(_) => T::from_column(row, column).map(Some),
        }
    }
}

/// A model field's value as a record attribute, for a write.
pub trait ToColumn {
    /// The value as an attribute.
    fn to_column(&self) -> AttrValue;
}

macro_rules! to_column_via_from {
    ($($ty:ty),*) => {$(
        impl ToColumn for $ty {
            fn to_column(&self) -> AttrValue {
                AttrValue::from(self.clone())
            }
        }
    )*};
}

to_column_via_from!(String, i64, f64, bool, DateTime<Utc>, serde_json::Value);

impl ToColumn for i32 {
    fn to_column(&self) -> AttrValue {
        AttrValue::Int(i64::from(*self))
    }
}

impl<T: ToColumn> ToColumn for Option<T> {
    fn to_column(&self) -> AttrValue {
        self.as_ref().map_or(AttrValue::Null, ToColumn::to_column)
    }
}

/// Every column of `M` cast to text, from its table: the select each read
/// starts from.
fn select<M: Model>(backend: crate::DbBackend) -> sea_query::SelectStatement {
    let mut select = Query::select();
    for column in M::COLUMNS {
        select.expr_as(
            Expr::col(Alias::new(*column)).cast_as(Alias::new(text_cast(backend))),
            Alias::new(*column),
        );
    }
    select.from(Alias::new(M::TABLE));
    select
}

async fn fetch<M: Model>(
    db: &Db,
    select: sea_query::SelectStatement,
) -> Result<Vec<M>, sqlx::Error> {
    let (sql, values) = build(db.backend, select);
    bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?
        .iter()
        .map(M::from_row)
        .collect()
}

/// The model with id `id`, or `None`.
pub async fn find<M: Model>(db: &Db, id: i64) -> Result<Option<M>, sqlx::Error> {
    let mut select = select::<M>(db.backend);
    select
        .and_where(Expr::col(Alias::new(M::ID)).eq(id))
        .limit(1);
    Ok(fetch(db, select).await?.into_iter().next())
}

/// Every row of the model's table, in id order.
pub async fn all<M: Model>(db: &Db) -> Result<Vec<M>, sqlx::Error> {
    let mut select = select::<M>(db.backend);
    select.order_by(Alias::new(M::ID), Order::Asc);
    fetch(db, select).await
}

/// The models with the given ids, by id, in one query.
pub async fn find_many<M: Model>(db: &Db, ids: &[i64]) -> Result<HashMap<i64, M>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut select = select::<M>(db.backend);
    select.and_where(Expr::col(Alias::new(M::ID)).is_in(ids.iter().copied()));
    Ok(fetch::<M>(db, select)
        .await?
        .into_iter()
        .map(|m| (m.id(), m))
        .collect())
}

/// The models whose `foreign_key` holds one of `parent_ids`, grouped by that
/// parent id, in one query: a has-many relation loaded for a whole page.
///
/// `parent_of` reads the foreign key back from a loaded model.
pub async fn has_many<M: Model>(
    db: &Db,
    foreign_key: &str,
    parent_ids: &[i64],
    parent_of: impl Fn(&M) -> i64,
) -> Result<HashMap<i64, Vec<M>>, sqlx::Error> {
    let mut grouped: HashMap<i64, Vec<M>> = HashMap::new();
    if parent_ids.is_empty() {
        return Ok(grouped);
    }
    let mut select = select::<M>(db.backend);
    select
        .and_where(Expr::col(Alias::new(foreign_key)).is_in(parent_ids.iter().copied()))
        .order_by(Alias::new(M::ID), Order::Asc);
    for model in fetch::<M>(db, select).await? {
        grouped.entry(parent_of(&model)).or_default().push(model);
    }
    Ok(grouped)
}

/// Inserts `model`, returning the new id.
pub async fn insert<M: Model>(db: &Db, model: &M) -> Result<i64, sqlx::Error> {
    let rec = model.to_record();
    let stmt = {
        let keys: Vec<&str> = rec.iter().map(|(k, _)| k).collect();
        Query::insert()
            .into_table(Alias::new(M::TABLE))
            .columns(keys.iter().map(|k| Alias::new(*k)))
            .values_panic(keys.iter().map(|k| bind(rec.get(k))))
            .to_owned()
    };
    insert_returning_id(db, stmt, Alias::new(M::ID)).await
}

/// Writes every column of `model` to the row with its id.
pub async fn update<M: Model>(db: &Db, model: &M) -> Result<(), sqlx::Error> {
    let rec = model.to_record();
    let (sql, values) = {
        let mut update = Query::update();
        update.table(Alias::new(M::TABLE));
        for (key, value) in rec.iter() {
            update.value(Alias::new(key), bind(Some(value)));
        }
        update.and_where(Expr::col(Alias::new(M::ID)).eq(model.id()));
        build(db.backend, update)
    };
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await
        .map(|_| ())
}

/// A record value as a bound expression, in the stored shape: a number as a
/// number, a bool as `1` or `0`, a datetime as UTC text, JSON as its text.
fn bind(value: Option<&AttrValue>) -> sea_query::SimpleExpr {
    match value {
        None | Some(AttrValue::Null) => sea_query::SimpleExpr::Keyword(sea_query::Keyword::Null),
        Some(AttrValue::Int(n)) => (*n).into(),
        Some(AttrValue::Bool(b)) => i32::from(*b).into(),
        Some(AttrValue::Float(f)) => (*f).into(),
        Some(other) => other.to_text().into(),
    }
}
