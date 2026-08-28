//! The framework-level error taxonomy.
//!
//! Application crates layer their own domain errors on top; anything that
//! crosses a framework boundary (handlers, modules, jobs) converges here so
//! HTTP mapping and logging stay uniform.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error")]
    Database(#[from] sqlx::Error),

    #[error("migration '{name}' in module '{module}' changed after being applied")]
    MigrationDrift { module: String, name: String },

    #[error("migration '{name}' in module '{module}' cannot be reversed")]
    Irreversible { module: String, name: String },

    #[error("module '{module}' registered more than once")]
    DuplicateModule { module: String },

    #[error("module '{module}' depends on unregistered module '{dependency}'")]
    UnknownModuleDependency { module: String, dependency: String },

    #[error("dependency cycle through module '{module}'")]
    ModuleDependencyCycle { module: String },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type CoreResult<T> = Result<T, CoreError>;
