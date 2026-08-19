//! Multi-backend test harness (feature `testing`).
//!
//! [`connect_test`] gives a test a fresh database on whichever backend the run
//! targets, so the same suite runs unchanged against SQLite, Postgres, and
//! MySQL. The backend is selected by the `LATERITE_TEST_DATABASE_URL`
//! environment variable (default `sqlite::memory:`), keeping credentials out of
//! the source tree.
//!
//! ```no_run
//! # async fn f() {
//! use laterite_core::testing::connect_test;
//! // Hold the guard for the whole test: it drops the ephemeral database at end.
//! let (db, _guard) = connect_test(&[/* module migration sets */]).await;
//! # let _ = db;
//! # }
//! ```

use sqlx::any::AnyPoolOptions;

use crate::migration::{run, DbBackend, MigrationSet};
use crate::Db;

/// Environment variable naming the maintenance database URL the harness targets.
const ENV_URL: &str = "LATERITE_TEST_DATABASE_URL";

/// Drops the ephemeral database created for a test when it goes out of scope.
/// For an in-memory SQLite run there is nothing to clean up and this is inert.
pub struct TestGuard {
    inner: Option<Cleanup>,
}

struct Cleanup {
    /// The maintenance connection used to run `drop database`.
    admin_url: String,
    /// The ephemeral database to drop.
    db_name: String,
    /// The backend, so Postgres can terminate connections before the drop.
    backend: DbBackend,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        let Some(cleanup) = self.inner.take() else {
            return;
        };
        // A `#[tokio::test]` runs on a current-thread runtime that cannot be
        // blocked on from within, so teardown runs on its own thread and runtime.
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("teardown runtime");
            rt.block_on(async move {
                // Drop the ephemeral database without closing the app pool:
                // `AnyPool::close` can hang on MySQL, and it is not needed. MySQL
                // drops a database with live connections; Postgres refuses, so we
                // first terminate any remaining backends on that database.
                if let Ok(admin) = AnyPoolOptions::new()
                    .max_connections(1)
                    .connect(&cleanup.admin_url)
                    .await
                {
                    if matches!(cleanup.backend, DbBackend::Postgres) {
                        let _ = sqlx::query(&format!(
                            "select pg_terminate_backend(pid) from pg_stat_activity \
                             where datname = '{}' and pid <> pg_backend_pid()",
                            cleanup.db_name
                        ))
                        .execute(&admin)
                        .await;
                    }
                    let _ = sqlx::query(&format!("drop database if exists {}", cleanup.db_name))
                        .execute(&admin)
                        .await;
                }
            });
        })
        .join();
    }
}

/// Connects to a fresh test database for the configured backend and applies
/// `migrations` through the framework runner (the same path an app uses at
/// startup). Returns the [`Db`] and a [`TestGuard`]; keep the guard alive for
/// the duration of the test.
///
/// The backend is chosen from `LATERITE_TEST_DATABASE_URL` (default
/// `sqlite::memory:`):
///
/// - **SQLite**: a private in-memory database, discarded when the pool drops.
/// - **Postgres / MySQL**: an ephemeral `laterite_test_<uuid>` database created
///   on the server named in the URL and dropped when the guard drops, so tests
///   are isolated and leave nothing behind.
pub async fn connect_test(migrations: &[MigrationSet]) -> (Db, TestGuard) {
    sqlx::any::install_default_drivers();
    let base = std::env::var(ENV_URL).unwrap_or_else(|_| "sqlite::memory:".to_string());
    let backend = DbBackend::from_url(&base).expect("recognised database URL");

    let (db, guard) = match backend {
        DbBackend::Sqlite => {
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("connect in-memory sqlite");
            (Db::new(pool, backend), TestGuard { inner: None })
        }
        DbBackend::Postgres | DbBackend::Mysql => {
            let db_name = format!("laterite_test_{}", uuid::Uuid::new_v4().simple());
            let admin = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&base)
                .await
                .expect("connect maintenance database");
            sqlx::query(&format!("create database {db_name}"))
                .execute(&admin)
                .await
                .expect("create ephemeral test database");
            admin.close().await;

            let pool = AnyPoolOptions::new()
                .max_connections(5)
                .connect(&swap_database(&base, &db_name))
                .await
                .expect("connect ephemeral test database");
            let guard = TestGuard {
                inner: Some(Cleanup {
                    admin_url: base,
                    db_name,
                    backend,
                }),
            };
            (Db::new(pool, backend), guard)
        }
    };

    run(&db.pool, db.backend, migrations)
        .await
        .expect("apply migrations to the test database");
    (db, guard)
}

/// Returns `base` with its database path replaced by `name`, so a maintenance
/// URL becomes a URL for the ephemeral test database on the same server.
fn swap_database(base: &str, name: &str) -> String {
    let mut url = url::Url::parse(base).expect("valid database url");
    url.set_path(name);
    url.into()
}
