//! Laterite core: the application kernel.
//!
//! The first crate of the Laterite content management framework. Provides
//! what every Laterite application starts from: layered configuration
//! loading, the framework error taxonomy, database connectivity and
//! migrations, module registration, and pagination primitives.

pub mod config;
pub mod db;
pub mod error;
pub mod migration;
pub mod module;
pub mod pagination;
pub mod query;
pub mod strata;
#[cfg(feature = "testing")]
pub mod testing;

pub use db::Db;
pub use error::{CoreError, CoreResult};
pub use migration::{bool_col, key_col, DbBackend, Migration, MigrationSet, Schema, SqlMigration};
pub use query::AnyRowExt;
