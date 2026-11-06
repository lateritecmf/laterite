//! The framework-level error taxonomy.
//!
//! Application crates layer their own domain errors on top; anything that
//! crosses a framework boundary (handlers, modules, jobs) converges here so
//! HTTP mapping and logging stay uniform.

use thiserror::Error;

/// An error any framework boundary can return: handlers, modules, jobs.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Produced when a configuration value is missing or malformed.
    #[error("configuration error: {0}")]
    Config(String),

    /// Produced when the database driver reports an error.
    #[error("database error")]
    Database(#[from] sqlx::Error),

    /// Names a migration whose source changed after it was applied.
    #[error("migration '{name}' in module '{module}' changed after being applied")]
    MigrationDrift {
        /// Id of the module that owns the migration.
        module: String,
        /// Name of the migration.
        name: String,
    },

    /// Produced when a rollback reaches a migration with no `down`.
    #[error("migration '{name}' in module '{module}' cannot be reversed")]
    Irreversible {
        /// Id of the module that owns the migration.
        module: String,
        /// Name of the migration.
        name: String,
    },

    /// Produced when two modules declare the same id.
    #[error("module '{module}' registered more than once")]
    DuplicateModule {
        /// The id registered twice.
        module: String,
    },

    /// Produced when a module depends on a module the application does not
    /// register.
    #[error("module '{module}' depends on unregistered module '{dependency}'")]
    UnknownModuleDependency {
        /// Id of the module declaring the dependency.
        module: String,
        /// Id of the missing module.
        dependency: String,
    },

    /// Produced when module dependencies form a cycle.
    #[error("dependency cycle through module '{module}'")]
    ModuleDependencyCycle {
        /// Id of a module on the cycle.
        module: String,
    },

    /// Produced when a module needs a database capability the connected
    /// backend lacks.
    #[error(
        "module '{module}' requires database capability '{capability}', unavailable on {backend}"
    )]
    MissingDbCapability {
        /// Id of the module that requires the capability.
        module: String,
        /// Name of the missing capability.
        capability: String,
        /// Name of the connected backend.
        backend: String,
    },

    /// Produced when a requested record or resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// Produced when input fails validation; the message says why.
    #[error("validation failed: {0}")]
    Validation(String),

    /// Produced when the request carries no valid session or credentials.
    #[error("unauthorized")]
    Unauthorized,

    /// Produced when the user lacks the permission an action requires.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// Produced when a write collides with existing state (a duplicate key).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Produced for a failure with no better category; the message says what.
    #[error("internal error: {0}")]
    Internal(String),
}

/// A result whose error is [`CoreError`].
pub type CoreResult<T> = Result<T, CoreError>;
