//! Database connectivity.
//!
//! The pool is `sqlx::Any`, so a deployment runs on Postgres, MySQL, or SQLite
//! by its configured URL. [`Db`] pairs the pool with the concrete [`DbBackend`]
//! (inferred from the URL) so the query and migration layers can render SQL for
//! the running backend.

use std::time::Duration;

use sqlx::any::{install_default_drivers, AnyPoolOptions};
use sqlx::AnyPool;

use crate::config::DatabaseConfig;
use crate::error::CoreResult;
use crate::migration::DbBackend;

/// An application database handle: the connection pool plus the backend it
/// speaks. Cheap to clone (the pool is reference-counted).
#[derive(Clone)]
pub struct Db {
    pub pool: AnyPool,
    pub backend: DbBackend,
}

impl Db {
    /// Wraps an existing pool with a known backend (used by tests).
    pub fn new(pool: AnyPool, backend: DbBackend) -> Self {
        Self { pool, backend }
    }
}

/// Creates the application database handle from configuration. Installs the
/// `sqlx::Any` drivers on first use so any supported backend can be dialled.
pub async fn connect(cfg: &DatabaseConfig) -> CoreResult<Db> {
    install_default_drivers();
    let backend = DbBackend::from_url(&cfg.url)?;
    let pool = AnyPoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(Duration::from_secs(cfg.acquire_timeout_secs))
        .connect(&cfg.url)
        .await?;
    Ok(Db { pool, backend })
}
