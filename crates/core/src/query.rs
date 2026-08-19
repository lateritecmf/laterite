//! Portable parameterised queries over `sqlx::Any`.
//!
//! `sea-query` builds the SQL and an ordered list of values for the running
//! backend; [`bind_values`] binds those values to a `sqlx::Any` query. Only
//! portable value kinds are supported (bool, integers, floats, text, bytes), so
//! backend-specific types (timestamps, JSON) are represented as text or integers
//! at the query boundary and converted in Rust.

use sqlx::any::{AnyArguments, AnyRow};
use sqlx::{Any, Encode, Row, Type};

use crate::migration::DbBackend;
use crate::Db;

/// Row helpers for portably stored types.
pub trait AnyRowExt {
    /// Reads a boolean stored as a 0/1 integer (the portable representation, see
    /// [`bind_values`]). The counterpart to binding a `bool`.
    fn get_bool(&self, column: &str) -> Result<bool, sqlx::Error>;
}

impl AnyRowExt for AnyRow {
    fn get_bool(&self, column: &str) -> Result<bool, sqlx::Error> {
        Ok(self.try_get::<i32, _>(column)? != 0)
    }
}

/// Renders a `sea-query` statement to `(sql, values)` for `backend`.
///
/// The statement is taken by value and dropped here, so no `sea-query` builder
/// (which holds non-`Send` reference-counted identifiers) survives into the
/// caller's `.await`. That keeps request handlers' futures `Send`, as the async
/// runtime requires. Construct the statement, hand it here, then bind and run
/// the returned owned `(sql, values)`.
pub fn build<S>(backend: DbBackend, stmt: S) -> (String, sea_query::Values)
where
    S: sea_query::QueryStatementWriter,
{
    match backend {
        DbBackend::Postgres => stmt.build(sea_query::PostgresQueryBuilder),
        DbBackend::Mysql => stmt.build(sea_query::MysqlQueryBuilder),
        DbBackend::Sqlite => stmt.build(sea_query::SqliteQueryBuilder),
    }
}

/// Runs an insert and returns the generated auto-increment id, portably.
///
/// The id column is `bigint auto_increment` on every backend, but reading the
/// new id back is not uniform: Postgres exposes no last-insert-id, so its
/// statement is given a `RETURNING` clause and the id is read from the returned
/// row; MySQL and SQLite report it through the driver after a plain execute.
/// `id` names the auto-increment column (used only for the Postgres `RETURNING`).
//
// This is a synchronous function returning a future, not an `async fn`: an
// `async fn` captures all its parameters into the future from the moment it is
// created, so taking the non-`Send` `sea-query` builder by value would make the
// future non-`Send` regardless of when the body drops it. Rendering the builder
// to owned `Send` data first, then capturing only that into an `async move`
// block, keeps the returned future `Send` as request handlers require.
pub fn insert_returning_id<I>(
    db: &Db,
    stmt: sea_query::InsertStatement,
    id: I,
) -> impl std::future::Future<Output = Result<i64, sqlx::Error>> + Send + '_
where
    I: sea_query::IntoIden + 'static,
{
    let (sql, values, returning) = render_insert(db.backend, stmt, id);
    async move {
        if returning {
            bind_values(sqlx::query(&sql), values)
                .fetch_one(&db.pool)
                .await?
                .try_get::<i64, _>(0)
        } else {
            bind_values(sqlx::query(&sql), values)
                .execute(&db.pool)
                .await?
                .last_insert_id()
                .ok_or(sqlx::Error::RowNotFound)
        }
    }
}

/// Renders an insert to `(sql, values, use_returning)`. Postgres and SQLite get a
/// `RETURNING` clause on `id` (read back with `fetch_one`); MySQL has no portable
/// `RETURNING`, so it reports the id through the driver's last-insert-id instead.
fn render_insert<I>(
    backend: DbBackend,
    mut stmt: sea_query::InsertStatement,
    id: I,
) -> (String, sea_query::Values, bool)
where
    I: sea_query::IntoIden + 'static,
{
    let returning = matches!(backend, DbBackend::Postgres | DbBackend::Sqlite);
    if returning {
        stmt.returning_col(id);
    }
    let (sql, values) = build(backend, stmt);
    (sql, values, returning)
}

type AnyQuery<'q> = sqlx::query::Query<'q, Any, AnyArguments<'q>>;
type AnyQueryAs<'q, O> = sqlx::query::QueryAs<'q, Any, O, AnyArguments<'q>>;

fn bind_one<'q, T>(query: AnyQuery<'q>, value: T) -> AnyQuery<'q>
where
    T: 'q + Send + Encode<'q, Any> + Type<Any>,
{
    query.bind(value)
}

/// Binds `sea-query` values onto a `sqlx::Any` query, in order. Only portable
/// value kinds occur here (the framework converts time/JSON to text before
/// building), so an unsupported kind is a framework bug and panics.
pub fn bind_values(mut query: AnyQuery<'_>, values: sea_query::Values) -> AnyQuery<'_> {
    use sea_query::Value;
    for value in values.0 {
        query = match value {
            // Booleans are stored as 0/1 integers everywhere (a SQLite `boolean`
            // column is not decodable through `sqlx::Any`), so a `bool` value
            // binds as an integer. Callers write `bool`; storage stays portable.
            Value::Bool(v) => bind_one(query, v.map(i32::from)),
            Value::TinyInt(v) => bind_one(query, v.map(i32::from)),
            Value::SmallInt(v) => bind_one(query, v),
            Value::Int(v) => bind_one(query, v),
            Value::BigInt(v) => bind_one(query, v),
            // `sqlx::Any` has no unsigned column types, so unsigned integers
            // (sea-query emits these for LIMIT/OFFSET) bind as the next wider
            // signed integer. The framework never stores unsigned values.
            Value::TinyUnsigned(v) => bind_one(query, v.map(i32::from)),
            Value::SmallUnsigned(v) => bind_one(query, v.map(i32::from)),
            Value::Unsigned(v) => bind_one(query, v.map(i64::from)),
            Value::BigUnsigned(v) => bind_one(query, v.map(|n| n as i64)),
            Value::Float(v) => bind_one(query, v),
            Value::Double(v) => bind_one(query, v),
            Value::String(v) => bind_one(query, v.map(|b| *b)),
            Value::Char(v) => bind_one(query, v.map(|c| c.to_string())),
            Value::Bytes(v) => bind_one(query, v.map(|b| *b)),
            // Defensive: unreachable under the framework's sea-query features, but
            // guards against a richer `Value` if a backend feature is unified in.
            #[allow(unreachable_patterns)]
            other => panic!("unsupported portable bind value: {other:?}"),
        };
    }
    query
}

/// Same as [`bind_values`], for a `query_as` mapping rows into `O`.
pub fn bind_values_as<O>(
    mut query: AnyQueryAs<'_, O>,
    values: sea_query::Values,
) -> AnyQueryAs<'_, O> {
    use sea_query::Value;
    for value in values.0 {
        query = match value {
            Value::Bool(v) => query.bind(v.map(i32::from)),
            Value::TinyInt(v) => query.bind(v.map(i32::from)),
            Value::SmallInt(v) => query.bind(v),
            Value::Int(v) => query.bind(v),
            Value::BigInt(v) => query.bind(v),
            Value::TinyUnsigned(v) => query.bind(v.map(i32::from)),
            Value::SmallUnsigned(v) => query.bind(v.map(i32::from)),
            Value::Unsigned(v) => query.bind(v.map(i64::from)),
            Value::BigUnsigned(v) => query.bind(v.map(|n| n as i64)),
            Value::Float(v) => query.bind(v),
            Value::Double(v) => query.bind(v),
            Value::String(v) => query.bind(v.map(|b| *b)),
            Value::Char(v) => query.bind(v.map(|c| c.to_string())),
            Value::Bytes(v) => query.bind(v.map(|b| *b)),
            #[allow(unreachable_patterns)]
            other => panic!("unsupported portable bind value: {other:?}"),
        };
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{Alias, Expr, Iden, Query};
    use sqlx::AnyPool;

    #[derive(Iden)]
    enum Widget {
        Table,
        Id,
        Label,
        Qty,
    }

    async fn pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "create table widget (id text primary key, label text not null, qty integer not null)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn binds_parameters_on_insert_and_select() {
        let pool = pool().await;
        let backend = DbBackend::Sqlite;

        let insert = Query::insert()
            .into_table(Widget::Table)
            .columns([Widget::Id, Widget::Label, Widget::Qty])
            .values_panic(["w-1".into(), "Sprocket".into(), 7.into()])
            .to_owned();
        let (sql, values) = build(backend, insert);
        bind_values(sqlx::query(&sql), values)
            .execute(&pool)
            .await
            .unwrap();

        // Select the label back by a bound id parameter.
        let select = Query::select()
            .column(Widget::Label)
            .from(Widget::Table)
            .and_where(Expr::col(Widget::Id).eq("w-1"))
            .to_owned();
        let (sql, values) = build(backend, select);
        let label: String = bind_values_as(sqlx::query_as::<_, (String,)>(&sql), values)
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
        assert_eq!(label, "Sprocket");

        // A cast-free count via a bound filter.
        let count_stmt = Query::select()
            .expr(Expr::col(Widget::Id).count())
            .from(Widget::Table)
            .and_where(Expr::col(Alias::new("qty")).eq(7))
            .to_owned();
        let (sql, values) = build(backend, count_stmt);
        let count: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&sql), values)
            .fetch_one(&pool)
            .await
            .unwrap()
            .0;
        assert_eq!(count, 1);
    }
}
