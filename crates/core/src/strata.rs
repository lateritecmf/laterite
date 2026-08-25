//! `strata`: the one-import toolkit for migrations and store queries.
//!
//! Migrations stack the schema in layers, the way rock strata build up, and this
//! module gathers everything a layer needs. `use laterite_core::strata::*;`
//! brings in the `Migration` trait and its `#[async_trait]` macro, the schema and
//! query builders, the `Db` handle, and the portability polyfills (`bool_col`,
//! `key_col`, `AnyRowExt`). Importing the general structure is enough; you should not have
//! to reach for individual items or remember which representation a type needs on
//! which backend. The `lat make:migration` scaffolding writes a file with this
//! import already in place.

pub use async_trait::async_trait;

pub use crate::error::{CoreError, CoreResult};
pub use crate::migration::{bool_col, key_col, Migration, MigrationSet, Schema, SqlMigration};
pub use crate::query::{
    bind_values, bind_values_as, build, insert_returning_id, on_conflict_ignore, text_cast,
    AnyRowExt,
};
pub use crate::{Db, DbBackend};

// Common `sea-query` items for building DDL and queries, re-exported so a
// migration or query file does not depend on `sea-query` directly.
pub use sea_query::{
    ColumnDef, Expr, ForeignKey, ForeignKeyAction, Iden, Index, OnConflict, Order, Query, Table,
};
