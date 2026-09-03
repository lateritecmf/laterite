//! The persister seam: a form's write is a handler, resolved at boot from a
//! string key on the descriptor.
//!
//! Every form gets a [`DefaultPersister`] (the descriptor-driven single-statement
//! insert/update) unless it names a registered [`Persister`], letting an entity
//! that needs a custom, atomic, multi-statement write (a tree's ancestor closure,
//! say) plug in without a hand-written CRUD handler. A persister is a registered
//! domain write service, the same category as a picker source.

use std::collections::HashMap;
use std::sync::Arc;

use laterite_core::query::{bind_values, build as to_sql, insert_returning_id, text_cast};
use laterite_core::strata::async_trait;
use laterite_core::validation::ErrorBag;
use laterite_core::Db;
use sea_query::{Alias, Expr, Query, SimpleExpr};

use crate::form::FormConfig;

/// Why a persist failed.
pub enum SaveError {
    /// A persist-time domain check (for example "re-parenting is not supported"):
    /// the form re-renders 422 with these per-field messages.
    Invalid(ErrorBag),
    /// The write itself failed: logged, and the form re-renders the generic
    /// banner.
    Failed(String),
}

/// Replaces the default insert/update for one form. An implementor owns its own
/// transaction when the write is multi-statement. `data` is the validated
/// submission (all text; `""` means empty/none).
#[async_trait]
pub trait Persister: Send + Sync + 'static {
    /// Persists a new record, returning its id.
    async fn create(&self, db: &Db, data: &HashMap<String, String>) -> Result<i64, SaveError>;
    /// Persists an edit to the record with primary key `id`.
    async fn update(
        &self,
        db: &Db,
        id: &str,
        data: &HashMap<String, String>,
    ) -> Result<(), SaveError>;
}

/// A registered persister: its dotted `vendor.name` key and the handler.
pub struct PersisterReg {
    pub name: String,
    pub persister: Arc<dyn Persister>,
}

impl PersisterReg {
    pub fn new(name: impl Into<String>, persister: Arc<dyn Persister>) -> Self {
        Self {
            name: name.into(),
            persister,
        }
    }
}

/// The persister registry, keyed by name.
pub type PersisterRegistry = HashMap<String, Arc<dyn Persister>>;

/// The built-in write: a descriptor-driven single-statement insert/update over
/// the form's own columns, what every form gets unless it names a custom one.
/// Trusts validated identifiers (the form guards them before calling).
pub(crate) struct DefaultPersister {
    entity: String,
    id_field: String,
    columns: Vec<String>,
}

impl DefaultPersister {
    pub(crate) fn from_config(config: &FormConfig) -> Self {
        Self {
            entity: config.entity.clone(),
            id_field: config.id_field.clone(),
            columns: config.fields.iter().map(|f| f.name.clone()).collect(),
        }
    }
}

#[async_trait]
impl Persister for DefaultPersister {
    async fn create(&self, db: &Db, data: &HashMap<String, String>) -> Result<i64, SaveError> {
        // The PK is a database-assigned auto-increment id, so the insert lists
        // only the descriptor's fields. Scope the builder so it drops before the
        // await, keeping the future `Send`.
        let stmt = {
            let vals: Vec<SimpleExpr> = self
                .columns
                .iter()
                .map(|c| data.get(c).cloned().unwrap_or_default().into())
                .collect();
            Query::insert()
                .into_table(Alias::new(&self.entity))
                .columns(self.columns.iter().map(Alias::new))
                .values_panic(vals)
                .to_owned()
        };
        insert_returning_id(db, stmt, Alias::new(&self.id_field))
            .await
            .map_err(|e| SaveError::Failed(e.to_string()))
    }

    async fn update(
        &self,
        db: &Db,
        id: &str,
        data: &HashMap<String, String>,
    ) -> Result<(), SaveError> {
        let (sql, values) = {
            let mut update = Query::update();
            update.table(Alias::new(&self.entity));
            for column in &self.columns {
                update.value(
                    Alias::new(column),
                    data.get(column).cloned().unwrap_or_default(),
                );
            }
            update.and_where(
                Expr::col(Alias::new(&self.id_field))
                    .cast_as(Alias::new(text_cast(db.backend)))
                    .eq(id),
            );
            to_sql(db.backend, update)
        };
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await
            .map(|_| ())
            .map_err(|e| SaveError::Failed(e.to_string()))
    }
}
