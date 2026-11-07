//! Seeders: data a module puts in the database, run again at will.
//!
//! A migration is schema and runs once; a seeder is data and runs whenever it
//! is asked, so it must leave the database the same however many times it runs.
//! [`insert_missing`] is the way to write one: it inserts a row only when no row
//! has the same natural key.

use sea_query::{Alias, Expr, Query};

use crate::query::{bind_values, build, insert_returning_id};
use crate::record::{AttrValue, Record};
use crate::{CoreResult, Db};

/// A unit of data a module seeds.
///
/// `run` must be idempotent: a second run changes nothing.
#[async_trait::async_trait]
pub trait Seeder: Send + Sync {
    /// The seeder's name, unique within its module, shown when it runs.
    fn name(&self) -> &str;
    /// Puts the data in the database.
    async fn run(&self, db: &Db) -> CoreResult<()>;
}

/// A module's seeders, in the order they run.
pub struct SeederSet {
    /// Id of the module the seeders belong to.
    pub module_id: &'static str,
    /// The seeders, in run order.
    pub seeders: Vec<Box<dyn Seeder>>,
}

impl SeederSet {
    /// The `seeders` of `module_id`.
    pub fn new(module_id: &'static str, seeders: Vec<Box<dyn Seeder>>) -> Self {
        Self { module_id, seeders }
    }
}

/// Runs every seeder in `sets`, or only those of the module `only`, in order.
/// Returns each `module_id/name` that ran.
pub async fn run(db: &Db, sets: &[SeederSet], only: Option<&str>) -> CoreResult<Vec<String>> {
    let mut ran = Vec::new();
    for set in sets {
        if only.is_some_and(|m| m != set.module_id) {
            continue;
        }
        for seeder in &set.seeders {
            seeder.run(db).await?;
            ran.push(format!("{}/{}", set.module_id, seeder.name()));
        }
    }
    Ok(ran)
}

/// Inserts `rec` into its table unless a row already holds the same values in
/// every column of `key`. Returns whether it inserted.
///
/// The way to write an idempotent seeder: name the row by its natural key
/// (a code, a slug, an email), never by an id.
pub async fn insert_missing(db: &Db, rec: &Record, key: &[&str]) -> CoreResult<bool> {
    let table = rec.entity();
    let (sql, values) = {
        let mut select = Query::select();
        select.expr(Expr::val(1)).from(Alias::new(table)).limit(1);
        for column in key {
            let value = rec
                .get(column)
                .and_then(AttrValue::to_text)
                .unwrap_or_default();
            select.and_where(Expr::col(Alias::new(*column)).eq(value));
        }
        build(db.backend, select)
    };
    let found = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    if found.is_some() {
        return Ok(false);
    }
    let stmt = {
        let keys: Vec<&str> = rec.iter().map(|(k, _)| k).collect();
        Query::insert()
            .into_table(Alias::new(table))
            .columns(keys.iter().map(|k| Alias::new(*k)))
            .values_panic(keys.iter().map(|k| bind(rec.get(k))))
            .to_owned()
    };
    insert_returning_id(db, stmt, Alias::new("id")).await?;
    Ok(true)
}

fn bind(value: Option<&AttrValue>) -> sea_query::SimpleExpr {
    match value {
        None | Some(AttrValue::Null) => sea_query::SimpleExpr::Keyword(sea_query::Keyword::Null),
        Some(AttrValue::Int(n)) => (*n).into(),
        Some(AttrValue::Float(f)) => (*f).into(),
        Some(AttrValue::Bool(b)) => i32::from(*b).into(),
        Some(other) => other.to_text().into(),
    }
}
