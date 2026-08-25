//! Portable, reversible migrations.
//!
//! A migration is a Rust unit implementing [`Migration`]: a stable `name`, an
//! `up`, and an optional `down` (absent means the migration is irreversible).
//! DDL is written with the `sea-query` schema builder through the [`Schema`]
//! handle, which renders for whichever backend the deployment runs on, so one
//! migration serves Postgres, MySQL, and SQLite. Raw SQL stays available as an
//! escape hatch, and [`SqlMigration`] wraps a pure-SQL up/down pair.
//!
//! A module exposes an ordered [`MigrationSet`] (its version history). The
//! runner applies pending migrations ([`run`]) and reverses them ([`rollback`],
//! [`reset`]), tracking what is applied by `(module_id, name)` in a single
//! `laterite_migrations` table. Queries run over `sqlx::Any`, so the same runner
//! drives every supported backend.

use async_trait::async_trait;
use sea_query::{
    ColumnDef, Expr, Iden, Index, MysqlQueryBuilder, PostgresQueryBuilder, Query,
    QueryStatementWriter, SchemaStatementBuilder, SqliteQueryBuilder, Table,
};
use sqlx::{AnyConnection, AnyPool};

use crate::error::{CoreError, CoreResult};

/// The database backend a deployment runs on. Selects the `sea-query` renderer
/// so migrations and the runner emit SQL the target understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbBackend {
    Postgres,
    Mysql,
    Sqlite,
}

impl DbBackend {
    /// Infers the backend from a connection URL scheme
    /// (`postgres://`, `mysql://`, `sqlite:`).
    pub fn from_url(url: &str) -> CoreResult<Self> {
        if url.starts_with("postgres") {
            Ok(Self::Postgres)
        } else if url.starts_with("mysql") {
            Ok(Self::Mysql)
        } else if url.starts_with("sqlite") {
            Ok(Self::Sqlite)
        } else {
            Err(CoreError::Config(format!(
                "unrecognised database URL scheme: {url}"
            )))
        }
    }
}

/// A portable boolean column, stored as a 0/1 integer. Use this in a migration
/// instead of `ColumnDef::new(name).boolean()`: `sqlx::Any` cannot decode a
/// SQLite `boolean` column, so integer is the representation that works on every
/// backend. Bind and read the value as a normal `bool` through the query layer
/// (see `crate::query`). Chain `.not_null()`, `.default(0)`, and so on as usual.
pub fn bool_col<T: sea_query::IntoIden>(name: T) -> ColumnDef {
    ColumnDef::new(name).integer().to_owned()
}

/// The length of a [`key_col`], generous enough for UUIDs, codes, usernames,
/// emails, and token hashes while staying within MySQL's index-length limit.
pub const KEY_LEN: u32 = 255;

/// A portable key column: a bounded `varchar` rather than `text`. Use this in a
/// migration for any column that participates in a key, index, or foreign key
/// (ids, codes, tokens, and columns named in an `Index`): MySQL cannot index a
/// `text` column without a prefix length, so a plain `text` id or unique column
/// fails there. A bounded string keys cleanly on every backend. It holds the
/// same UTF-8 strings a `text` column would, so application code is unchanged.
/// Chain `.not_null()`, `.primary_key()`, `.unique_key()`, and so on as usual.
pub fn key_col<T: sea_query::IntoIden>(name: T) -> ColumnDef {
    ColumnDef::new(name).string_len(KEY_LEN).to_owned()
}

fn schema_sql<S: SchemaStatementBuilder>(backend: DbBackend, stmt: &S) -> String {
    match backend {
        DbBackend::Postgres => stmt.build(PostgresQueryBuilder),
        DbBackend::Mysql => stmt.build(MysqlQueryBuilder),
        DbBackend::Sqlite => stmt.build(SqliteQueryBuilder),
    }
}

fn query_sql<Q: QueryStatementWriter>(backend: DbBackend, stmt: &Q) -> String {
    match backend {
        DbBackend::Postgres => stmt.to_string(PostgresQueryBuilder),
        DbBackend::Mysql => stmt.to_string(MysqlQueryBuilder),
        DbBackend::Sqlite => stmt.to_string(SqliteQueryBuilder),
    }
}

/// The handle a migration uses to change the schema. Renders `sea-query`
/// statements for the running backend, with a raw-SQL escape hatch.
pub struct Schema<'c> {
    conn: &'c mut AnyConnection,
    backend: DbBackend,
}

impl Schema<'_> {
    pub fn backend(&self) -> DbBackend {
        self.backend
    }

    /// Runs a `sea-query` schema statement (`Table::create/alter/drop`,
    /// `Index::create`, ...). Takes the statement by value and renders it to SQL
    /// before awaiting, so the (non-`Send`) statement is not held across the
    /// await point.
    pub async fn exec<S: SchemaStatementBuilder>(&mut self, stmt: S) -> CoreResult<()> {
        let sql = schema_sql(self.backend, &stmt);
        drop(stmt);
        sqlx::query(&sql).execute(&mut *self.conn).await?;
        Ok(())
    }

    /// Runs raw SQL, for anything the builder does not express. Portability is
    /// the caller's responsibility.
    pub async fn raw(&mut self, sql: &str) -> CoreResult<()> {
        sqlx::query(sql).execute(&mut *self.conn).await?;
        Ok(())
    }
}

/// One migration: a stable name, an `up`, and an optional `down`.
#[async_trait(?Send)]
pub trait Migration: Send + Sync {
    /// The stable name recorded in the tracking table. Never change it once the
    /// migration has shipped.
    fn name(&self) -> &str;

    /// Applies the migration.
    async fn up(&self, schema: &mut Schema<'_>) -> CoreResult<()>;

    /// Reverses the migration. The default marks it irreversible; the runner
    /// fills in the module id. Override to make a migration reversible.
    async fn down(&self, _schema: &mut Schema<'_>) -> CoreResult<()> {
        Err(CoreError::Irreversible {
            module: String::new(),
            name: self.name().to_string(),
        })
    }
}

/// A migration whose up and down are raw SQL strings. `down` is optional; its
/// absence makes the migration irreversible.
pub struct SqlMigration {
    name: String,
    up: String,
    down: Option<String>,
}

impl SqlMigration {
    pub fn new(name: impl Into<String>, up: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            up: up.into(),
            down: None,
        }
    }

    pub fn reversible(mut self, down: impl Into<String>) -> Self {
        self.down = Some(down.into());
        self
    }
}

#[async_trait(?Send)]
impl Migration for SqlMigration {
    fn name(&self) -> &str {
        &self.name
    }

    async fn up(&self, schema: &mut Schema<'_>) -> CoreResult<()> {
        schema.raw(&self.up).await
    }

    async fn down(&self, schema: &mut Schema<'_>) -> CoreResult<()> {
        match &self.down {
            Some(sql) => schema.raw(sql).await,
            None => Err(CoreError::Irreversible {
                module: String::new(),
                name: self.name.clone(),
            }),
        }
    }
}

/// A module's ordered migrations: the version history for `module_id`.
pub struct MigrationSet {
    pub module_id: &'static str,
    pub migrations: Vec<Box<dyn Migration>>,
}

impl MigrationSet {
    pub fn new(module_id: &'static str, migrations: Vec<Box<dyn Migration>>) -> Self {
        Self {
            module_id,
            migrations,
        }
    }
}

/// Declares a module's migration set from one-file migrations.
///
/// Each migration lives in its own file under the module's `migrations/`
/// directory, named `m<name>.rs` and exposing a `pub struct Migration` that
/// implements [`Migration`]. This macro, invoked in the directory's `mod.rs`,
/// lists them in apply order: it declares each file as a submodule and generates
/// the module's `MODULE_ID` constant and its `migrations()` function returning the
/// ordered [`MigrationSet`]. The list is the version history; append to its end,
/// never reorder or rename a shipped entry.
///
/// The `m` prefix lets a name start with a digit (a bare `0001_...` is not a
/// valid Rust identifier). Scaffold new entries with `lat make:migration`.
///
/// ```ignore
/// // src/migrations/mod.rs
/// laterite_core::migration_set! {
///     module_id: "acme.blog",
///     m0001_create_posts,
///     m0002_create_comments,
/// }
/// ```
#[macro_export]
macro_rules! migration_set {
    (module_id: $id:literal, $($m:ident,)*) => {
        $( mod $m; )*

        /// The stable migration namespace for this module. Never change it once
        /// migrations have shipped: applied history is keyed on it.
        pub const MODULE_ID: &str = $id;

        /// This module's migrations, in apply order.
        pub fn migrations() -> $crate::MigrationSet {
            $crate::MigrationSet::new(
                MODULE_ID,
                ::std::vec![ $( ::std::boxed::Box::new($m::Migration), )* ],
            )
        }
    };
}

#[derive(Iden)]
enum LateriteMigrations {
    Table,
    ModuleId,
    Name,
}

async fn ensure_tracking_table(pool: &AnyPool, backend: DbBackend) -> CoreResult<()> {
    let stmt = Table::create()
        .table(LateriteMigrations::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(LateriteMigrations::ModuleId)
                .string_len(255)
                .not_null(),
        )
        .col(
            ColumnDef::new(LateriteMigrations::Name)
                .string_len(255)
                .not_null(),
        )
        .primary_key(
            Index::create()
                .col(LateriteMigrations::ModuleId)
                .col(LateriteMigrations::Name),
        )
        .to_owned();
    sqlx::query(&schema_sql(backend, &stmt))
        .execute(pool)
        .await?;
    Ok(())
}

async fn is_applied(
    pool: &AnyPool,
    backend: DbBackend,
    module_id: &str,
    name: &str,
) -> CoreResult<bool> {
    let stmt = Query::select()
        .column(LateriteMigrations::Name)
        .from(LateriteMigrations::Table)
        .and_where(Expr::col(LateriteMigrations::ModuleId).eq(module_id))
        .and_where(Expr::col(LateriteMigrations::Name).eq(name))
        .limit(1)
        .to_owned();
    let found: Option<String> = sqlx::query_scalar(&query_sql(backend, &stmt))
        .fetch_optional(pool)
        .await?;
    Ok(found.is_some())
}

/// The names applied for a module, in application order.
pub async fn applied(
    pool: &AnyPool,
    backend: DbBackend,
    module_id: &str,
) -> CoreResult<Vec<String>> {
    ensure_tracking_table(pool, backend).await?;
    let stmt = Query::select()
        .column(LateriteMigrations::Name)
        .from(LateriteMigrations::Table)
        .and_where(Expr::col(LateriteMigrations::ModuleId).eq(module_id))
        .order_by(LateriteMigrations::Name, sea_query::Order::Asc)
        .to_owned();
    let names: Vec<String> = sqlx::query_scalar(&query_sql(backend, &stmt))
        .fetch_all(pool)
        .await?;
    Ok(names)
}

/// Applies every pending migration across the given sets, in listed order and,
/// within a set, in declared order. Each migration runs in its own transaction.
pub async fn run(pool: &AnyPool, backend: DbBackend, sets: &[MigrationSet]) -> CoreResult<()> {
    ensure_tracking_table(pool, backend).await?;
    for set in sets {
        for migration in &set.migrations {
            if is_applied(pool, backend, set.module_id, migration.name()).await? {
                continue;
            }
            let mut tx = pool.begin().await?;
            {
                let mut schema = Schema {
                    conn: &mut tx,
                    backend,
                };
                migration.up(&mut schema).await?;
            }
            let insert = Query::insert()
                .into_table(LateriteMigrations::Table)
                .columns([LateriteMigrations::ModuleId, LateriteMigrations::Name])
                .values_panic([set.module_id.into(), migration.name().into()])
                .to_owned();
            sqlx::query(&query_sql(backend, &insert))
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
    }
    Ok(())
}

/// Reverses the last `steps` applied migrations of one module, most recent
/// first. An irreversible migration in the way stops the rollback.
pub async fn rollback(
    pool: &AnyPool,
    backend: DbBackend,
    set: &MigrationSet,
    steps: usize,
) -> CoreResult<()> {
    ensure_tracking_table(pool, backend).await?;
    let mut done = 0;
    for migration in set.migrations.iter().rev() {
        if done >= steps {
            break;
        }
        if !is_applied(pool, backend, set.module_id, migration.name()).await? {
            continue;
        }
        let mut tx = pool.begin().await?;
        {
            let mut schema = Schema {
                conn: &mut tx,
                backend,
            };
            migration.down(&mut schema).await.map_err(|e| match e {
                CoreError::Irreversible { name, .. } => CoreError::Irreversible {
                    module: set.module_id.to_string(),
                    name,
                },
                other => other,
            })?;
        }
        let delete = Query::delete()
            .from_table(LateriteMigrations::Table)
            .and_where(Expr::col(LateriteMigrations::ModuleId).eq(set.module_id))
            .and_where(Expr::col(LateriteMigrations::Name).eq(migration.name()))
            .to_owned();
        sqlx::query(&query_sql(backend, &delete))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        done += 1;
    }
    Ok(())
}

/// Reverses every applied migration of one module.
pub async fn reset(pool: &AnyPool, backend: DbBackend, set: &MigrationSet) -> CoreResult<()> {
    rollback(pool, backend, set, set.migrations.len()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Iden)]
    enum Demo {
        Table,
        Id,
    }

    struct CreateDemo;

    #[async_trait(?Send)]
    impl Migration for CreateDemo {
        fn name(&self) -> &str {
            "0001_create_demo"
        }
        async fn up(&self, schema: &mut Schema<'_>) -> CoreResult<()> {
            schema
                .exec(
                    Table::create()
                        .table(Demo::Table)
                        .if_not_exists()
                        .col(ColumnDef::new(Demo::Id).integer().not_null())
                        .to_owned(),
                )
                .await
        }
        async fn down(&self, schema: &mut Schema<'_>) -> CoreResult<()> {
            schema
                .exec(Table::drop().table(Demo::Table).to_owned())
                .await
        }
    }

    async fn sqlite_pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn applies_and_rolls_back_on_sqlite() {
        let pool = sqlite_pool().await;
        let backend = DbBackend::Sqlite;
        let set = MigrationSet::new("test.demo", vec![Box::new(CreateDemo)]);

        run(&pool, backend, std::slice::from_ref(&set))
            .await
            .unwrap();
        // The table exists and rerunning is a no-op.
        run(&pool, backend, std::slice::from_ref(&set))
            .await
            .unwrap();
        sqlx::query("insert into demo (id) values (1)")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(applied(&pool, backend, "test.demo").await.unwrap().len(), 1);

        reset(&pool, backend, &set).await.unwrap();
        // The table is gone and the tracking row is cleared.
        assert!(sqlx::query("select count(*) from demo")
            .fetch_one(&pool)
            .await
            .is_err());
        assert!(applied(&pool, backend, "test.demo")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn irreversible_migration_reports_module_and_name() {
        let pool = sqlite_pool().await;
        let backend = DbBackend::Sqlite;
        let set = MigrationSet::new(
            "test.oneway",
            vec![Box::new(SqlMigration::new(
                "0001_make_t",
                "create table t (id integer not null)",
            ))],
        );
        run(&pool, backend, std::slice::from_ref(&set))
            .await
            .unwrap();
        let err = rollback(&pool, backend, &set, 1).await.unwrap_err();
        match err {
            CoreError::Irreversible { module, name } => {
                assert_eq!(module, "test.oneway");
                assert_eq!(name, "0001_make_t");
            }
            other => panic!("expected Irreversible, got {other:?}"),
        }
    }
}
