//! Database connectivity.

use std::time::Duration;

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
