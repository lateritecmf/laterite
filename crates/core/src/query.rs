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

    /// Reads a text column as a `String`, portably. MySQL reports `TEXT` columns
    /// as `BLOB` through `sqlx::Any`, so a plain `String` decode fails there; this
    /// falls back to reading the bytes and decoding UTF-8. Use for every text read
    /// so the same code works on every backend.
    fn get_text(&self, column: &str) -> Result<String, sqlx::Error>;

    /// The nullable counterpart to [`get_text`], for a `text` column that may be
    /// `NULL`.
    fn get_text_opt(&self, column: &str) -> Result<Option<String>, sqlx::Error>;

    /// Reads an integer column as `i64`, portably. `sqlx::Any` reports SQLite
    /// integers as `i64` but Postgres `INTEGER` as `i32`, so a fixed width fails
    /// on one backend; this tries `i64` then falls back to `i32`. Use for any
    /// integer column whatever its declared width, so a column need not be widened
    /// to `bigint` just to be read back.
    fn get_int(&self, column: &str) -> Result<i64, sqlx::Error>;

    /// The nullable counterpart to [`get_int`].
    fn get_int_opt(&self, column: &str) -> Result<Option<i64>, sqlx::Error>;
}

impl AnyRowExt for AnyRow {
    fn get_bool(&self, column: &str) -> Result<bool, sqlx::Error> {
        Ok(self.try_get::<i32, _>(column)? != 0)
    }

    fn get_text(&self, column: &str) -> Result<String, sqlx::Error> {
        match self.try_get::<String, _>(column) {
            Ok(s) => Ok(s),
            Err(_) => decode_utf8(column, self.try_get::<Vec<u8>, _>(column)?),
        }
    }

    fn get_text_opt(&self, column: &str) -> Result<Option<String>, sqlx::Error> {
        match self.try_get::<Option<String>, _>(column) {
            Ok(s) => Ok(s),
            Err(_) => self
                .try_get::<Option<Vec<u8>>, _>(column)?
                .map(|b| decode_utf8(column, b))
                .transpose(),
        }
    }

    fn get_int(&self, column: &str) -> Result<i64, sqlx::Error> {
        match self.try_get::<i64, _>(column) {
            Ok(v) => Ok(v),
            Err(_) => Ok(i64::from(self.try_get::<i32, _>(column)?)),
        }
    }

    fn get_int_opt(&self, column: &str) -> Result<Option<i64>, sqlx::Error> {
        match self.try_get::<Option<i64>, _>(column) {
            Ok(v) => Ok(v),
            Err(_) => Ok(self.try_get::<Option<i32>, _>(column)?.map(i64::from)),
        }
    }
}

/// Decodes bytes read from a text-typed column (the MySQL `BLOB` fallback path)
/// as UTF-8, surfacing a bad encoding as a column-decode error.
fn decode_utf8(column: &str, bytes: Vec<u8>) -> Result<String, sqlx::Error> {
    String::from_utf8(bytes).map_err(|e| sqlx::Error::ColumnDecode {
        index: column.to_string(),
        source: Box::new(e),
    })
}

/// A portable "insert, ignore duplicates" conflict clause for `keys` (the unique
/// or primary-key columns of the conflict). Use in place of
/// `OnConflict::columns(keys).do_nothing()`: sea-query renders MySQL's
/// `DO NOTHING` as invalid SQL (`ON DUPLICATE KEY IGNORE`), so this expresses the
/// same intent as a no-op update of the first key column, which is valid on
/// Postgres, MySQL, and SQLite alike.
pub fn on_conflict_ignore<C>(keys: impl IntoIterator<Item = C>) -> sea_query::OnConflict
where
    C: sea_query::IntoIden,
{
    use sea_query::IntoIden;
    let keys: Vec<sea_query::DynIden> = keys.into_iter().map(IntoIden::into_iden).collect();
    let first = keys[0].clone();
    sea_query::OnConflict::columns(keys)
        .update_column(first)
        .to_owned()
}

/// The SQL type to cast a column to when you need its value back as a string on
/// any backend. Descriptor-driven screens cast every selected column to a string
/// so a value of any type reads back uniformly through `sqlx::Any`. MySQL rejects
/// `CAST(x AS text)` (it casts to `char`), while Postgres and SQLite use `text`.
/// Use as `Expr::col(c).cast_as(sea_query::Alias::new(text_cast(backend)))`.
pub fn text_cast(backend: DbBackend) -> &'static str {
    match backend {
        DbBackend::Mysql => "char",
        DbBackend::Postgres | DbBackend::Sqlite => "text",
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

/// Like [`insert_returning_id`] but on an explicit connection (a transaction), so
/// a multi-statement write stays atomic. Sync-renders then captures only owned
/// `Send` data plus the connection into the future, for the same reason (see
/// [`insert_returning_id`]).
pub fn insert_returning_id_on<'c, I>(
    backend: DbBackend,
    conn: &'c mut sqlx::AnyConnection,
    stmt: sea_query::InsertStatement,
    id: I,
) -> impl std::future::Future<Output = Result<i64, sqlx::Error>> + Send + 'c
where
    I: sea_query::IntoIden + 'static,
{
    let (sql, values, returning) = render_insert(backend, stmt, id);
    async move {
        if returning {
            bind_values(sqlx::query(&sql), values)
                .fetch_one(&mut *conn)
                .await?
                .try_get::<i64, _>(0)
        } else {
            bind_values(sqlx::query(&sql), values)
                .execute(&mut *conn)
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

    // Runs on the full backend matrix: the id read-back differs per backend
    // (Postgres RETURNING vs MySQL last-insert-id), so it uses the portable test
    // harness + schema builder rather than raw, SQLite-only DDL.
    #[cfg(feature = "testing")]
    #[tokio::test]
    async fn insert_returning_id_on_runs_inside_a_transaction() {
        use crate::strata::{
            async_trait, ColumnDef, CoreResult, Migration, MigrationSet, Schema, Table,
        };
        use crate::testing::connect_test;

        struct CreateThing;
        #[async_trait(?Send)]
        impl Migration for CreateThing {
            fn name(&self) -> &str {
                "0001_create_thing"
            }
            async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
                s.exec(
                    Table::create()
                        .table(Alias::new("thing"))
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Alias::new("id"))
                                .big_integer()
                                .not_null()
                                .auto_increment()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(Alias::new("name")).text().not_null())
                        .to_owned(),
                )
                .await
            }
        }

        let (db, _guard) =
            connect_test(&[MigrationSet::new("test.thing", vec![Box::new(CreateThing)])]).await;

        // Insert inside a transaction, reading the new id back through the tx.
        let mut tx = db.pool.begin().await.unwrap();
        let insert = Query::insert()
            .into_table(Alias::new("thing"))
            .columns([Alias::new("name")])
            .values_panic(["gadget".into()])
            .to_owned();
        let id = insert_returning_id_on(db.backend, &mut tx, insert, Alias::new("id"))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(id >= 1);

        // The committed row is readable by the returned id.
        let select = Query::select()
            .column(Alias::new("name"))
            .from(Alias::new("thing"))
            .and_where(Expr::col(Alias::new("id")).eq(id))
            .to_owned();
        let (sql, values) = build(db.backend, select);
        let name: String = bind_values_as(sqlx::query_as::<_, (String,)>(&sql), values)
            .fetch_one(&db.pool)
            .await
            .unwrap()
            .0;
        assert_eq!(name, "gadget");
    }
}
