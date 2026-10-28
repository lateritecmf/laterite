//! Laterite core: the application kernel.
//!
//! The first crate of the Laterite content management framework. Provides
//! what every Laterite application starts from: layered configuration
//! loading, the framework error taxonomy, database connectivity and
//! migrations, module registration, and pagination primitives.

pub mod arrange;
pub mod cache;
pub mod capabilities;
pub mod config;
pub mod crypt;
pub mod db;
pub mod error;
pub mod events;
pub mod i18n;
pub mod icons;
pub mod listeners;
pub mod mail;
pub mod markdown;
pub mod migration;
pub mod module;
pub mod pagination;
pub mod parameters;
pub mod query;
pub mod queue;
pub mod record;
pub mod registry;
pub mod schedule;
pub mod search;
pub mod site;
pub mod strata;
#[cfg(feature = "testing")]
pub mod testing;
pub mod theme;
pub mod validation;

pub use cache::Cache;
pub use capabilities::CapabilitySet;
pub use crypt::{Crypt, CryptError, MasterKey};
pub use db::Db;
pub use error::{CoreError, CoreResult};
pub use events::{Event, EventCx, EventError, EventListenerReg, Events, Listener};
pub use i18n::{
    collect_sources, plural_category, Arg, CatalogStore, PluralCategory, Text, Translator,
    PSEUDO_LOCALE,
};
pub use laterite_macros::{t, tn, tp};
pub use listeners::Timestamps;
pub use mail::Mail;
pub use migration::{bool_col, key_col, DbBackend, Migration, MigrationSet, Schema, SqlMigration};
pub use module::{BootCx, Capability, Module, ModuleId, ModuleRegistry, SiteFilesCx, StaticSeedCx};
pub use query::AnyRowExt;
pub use queue::Queue;
pub use record::{
    Actor, AttrValue, ListenerTarget, ModelListener, ModelListenerReg, Op, Record, SaveCx, SavedCx,
};
pub use registry::{ContributeMode, ContributeOpts, Contribution, Registry};
pub use schedule::Schedule;
pub use site::{SiteContext, SiteFile};
pub use validation::{validate, validate_fields, ErrorBag, FieldRules, Mode, Rule};
