//! Laterite core: the application kernel.
//!
//! The first crate of the Laterite content management framework. Provides
//! what every Laterite application starts from: layered configuration
//! loading, the framework error taxonomy, database connectivity and
//! migrations, module registration, and pagination primitives.

pub mod config;
pub mod db;
pub mod error;
pub mod migrate;
pub mod module;
pub mod pagination;

pub use error::{CoreError, CoreResult};
pub use migrate::ModuleMigrations;
