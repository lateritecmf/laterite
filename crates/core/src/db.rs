//! Database connectivity and migrations.

use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::config::DatabaseConfig;
use crate::error::CoreResult;

/// Creates the application connection pool from configuration.
pub async fn connect(cfg: &DatabaseConfig) -> CoreResult<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(Duration::from_secs(cfg.acquire_timeout_secs))
        .connect(&cfg.url)
        .await?;
    Ok(pool)
}

/// Runs the application's migrations.
///
/// One migrator per application for now: sqlx records applied versions in a
/// single `_sqlx_migrations` table, so per-module migration directories need
/// version namespacing that is deferred until a second migration source
/// actually exists (pull, don't push).
pub async fn run_migrations(pool: &PgPool, migrator: &Migrator) -> CoreResult<()> {
    migrator.run(pool).await?;
    Ok(())
}
