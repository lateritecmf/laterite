//! Laterite core: the application kernel.
//!
//! The first crate of the Laterite content management framework. Provides
//! what every Laterite application starts from: layered configuration
//! loading, the framework error taxonomy, database connectivity and
//! migrations, module registration, and pagination primitives.

pub mod capabilities;
pub mod config;
pub mod db;
pub mod error;
pub mod i18n;
pub mod migration;
pub mod module;
pub mod pagination;
pub mod query;
pub mod registry;
pub mod strata;
#[cfg(feature = "testing")]
pub mod testing;
pub mod validation;

pub use capabilities::CapabilitySet;
pub use db::Db;
pub use error::{CoreError, CoreResult};
pub use i18n::Translator;
pub use migration::{bool_col, key_col, DbBackend, Migration, MigrationSet, Schema, SqlMigration};
pub use module::{Capability, Module, ModuleId, ModuleRegistry};
pub use query::AnyRowExt;
pub use registry::{ContributeMode, ContributeOpts, Contribution, Registry};
pub use validation::{validate, validate_fields, ErrorBag, FieldRules, Mode, Rule};
