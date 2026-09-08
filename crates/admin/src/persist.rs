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
use laterite_core::{AttrValue, Db, ModelListener, Op, Record};
use sea_query::{Alias, Expr, Query, SimpleExpr};

use crate::form::FormConfig;

/// Why a persist failed.
#[derive(Debug)]
pub enum SaveError {
    /// A persist-time domain check (for example "re-parenting is not supported"):
    /// the form re-renders 422 with these per-field messages.
    Invalid(ErrorBag),
    /// The write itself failed: logged, and the form re-renders the generic
    /// banner.
    Failed(String),
}

/// Replaces the default insert/update for one form. An implementor owns its own
/// transaction when the write is multi-statement. `rec` holds the validated
/// submission plus anything the listeners added; `rec.to_text_map()` gives the
/// all-text shape if that suits an implementation better.
#[async_trait]
pub trait Persister: Send + Sync + 'static {
    /// Persists a new record, returning its id.
    async fn create(&self, db: &Db, rec: &Record) -> Result<i64, SaveError>;
    /// Persists an edit to the record with primary key `id`.
    async fn update(&self, db: &Db, id: &str, rec: &Record) -> Result<(), SaveError>;
}

/// Binds one attribute for the storage layer.
///
/// Absent, null, and empty text all bind SQL `NULL`, so an optional non-text
/// column stores correctly instead of failing on `""`. Booleans bind as integers
/// (the one representation every backend indexes), and timestamps and JSON bind
/// as text.
fn bind(value: Option<&AttrValue>) -> SimpleExpr {
    match value {
        None | Some(AttrValue::Null) => Option::<String>::None.into(),
        Some(AttrValue::Int(i)) => (*i).into(),
        Some(AttrValue::Float(f)) => (*f).into(),
        Some(AttrValue::Bool(b)) => i32::from(*b).into(),
        Some(AttrValue::Text(s)) if s.is_empty() => Option::<String>::None.into(),
        Some(other) => other.to_text().into(),
    }
}

/// Runs the listeners around a persister write: each `before_save` may change
/// the record or refuse the write, then the persister writes, then each
/// `after_save` runs. `after_save` is not atomic with the write.
///
/// `id` is the row to update, and is ignored on a create.
pub(crate) async fn save(
    db: &Db,
    listeners: &[Arc<dyn ModelListener>],
    persister: &dyn Persister,
    rec: &mut Record,
    op: Op,
    id: Option<&str>,
) -> Result<(), SaveError> {
    for listener in listeners {
        listener
            .before_save(db, rec, op)
            .await
            .map_err(SaveError::Invalid)?;
    }

    match op {
        Op::Create => rec.set_id(persister.create(db, rec).await?),
        Op::Update => {
            let id = id.ok_or_else(|| SaveError::Failed("update without an id".to_string()))?;
            persister.update(db, id, rec).await?;
        }
        // `Op` is non-exhaustive; a stage added later needs its arm here.
        _ => return Err(SaveError::Failed(format!("unsupported operation {op:?}"))),
    }

    for listener in listeners {
        listener.after_save(db, rec, op).await;
    }
    Ok(())
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
    /// The columns to write: the descriptor's own fields, plus any attribute a
    /// listener added, minus the database-assigned id. Widening this way means a
    /// listener-injected `created_at` is written rather than silently dropped;
    /// the form admits only descriptor fields, so nothing a request submitted can
    /// reach a column it did not declare.
    fn write_columns(&self, rec: &Record) -> Vec<String> {
        let mut cols = self.columns.clone();
        for (key, _) in rec.iter() {
            if key != self.id_field && !cols.iter().any(|c| c == key) {
                cols.push(key.to_string());
            }
        }
        cols.retain(|c| c != &self.id_field && crate::sql::valid_ident(c));
        cols
    }

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
    async fn create(&self, db: &Db, rec: &Record) -> Result<i64, SaveError> {
        // The PK is database-assigned, so the insert never lists it. Scope the
        // builder so it drops before the await, keeping the future `Send`.
        let stmt = {
            let cols = self.write_columns(rec);
            let vals: Vec<SimpleExpr> = cols.iter().map(|c| bind(rec.get(c))).collect();
            Query::insert()
                .into_table(Alias::new(&self.entity))
                .columns(cols.iter().map(Alias::new))
                .values_panic(vals)
                .to_owned()
        };
        insert_returning_id(db, stmt, Alias::new(&self.id_field))
            .await
            .map_err(|e| SaveError::Failed(e.to_string()))
    }

    async fn update(&self, db: &Db, id: &str, rec: &Record) -> Result<(), SaveError> {
        let (sql, values) = {
            let mut update = Query::update();
            update.table(Alias::new(&self.entity));
            for column in self.write_columns(rec) {
                update.value(Alias::new(&column), bind(rec.get(&column)));
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

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use laterite_core::testing::connect_test;
    use laterite_core::{ListenerTarget, ModelListenerReg, Text};
    use std::sync::Mutex;

    /// Records what reached the persister, so a test can assert on the write.
    #[derive(Default)]
    struct SpyPersister {
        seen: Mutex<Vec<(String, HashMap<String, String>)>>,
    }

    #[async_trait]
    impl Persister for SpyPersister {
        async fn create(&self, _db: &Db, rec: &Record) -> Result<i64, SaveError> {
            self.seen
                .lock()
                .unwrap()
                .push(("create".into(), rec.to_text_map()));
            Ok(42)
        }
        async fn update(&self, _db: &Db, id: &str, rec: &Record) -> Result<(), SaveError> {
            self.seen
                .lock()
                .unwrap()
                .push((format!("update:{id}"), rec.to_text_map()));
            Ok(())
        }
    }

    /// Sets an attribute before the write, and appends its name to a shared log
    /// after it.
    struct Stamp {
        key: &'static str,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl ModelListener for Stamp {
        async fn before_save(&self, _db: &Db, rec: &mut Record, _op: Op) -> Result<(), ErrorBag> {
            rec.set(self.key, "set");
            Ok(())
        }
        async fn after_save(&self, _db: &Db, _rec: &Record, _op: Op) {
            self.log.lock().unwrap().push(self.key);
        }
    }

    /// Refuses every write.
    struct Veto;

    #[async_trait]
    impl ModelListener for Veto {
        async fn before_save(&self, _db: &Db, _rec: &mut Record, _op: Op) -> Result<(), ErrorBag> {
            let mut bag = ErrorBag::default();
            // A plain Text, not `t!`: a test fixture must not enter the catalog.
            bag.add("name", Text::new("Not allowed"));
            Err(bag)
        }
    }

    fn listeners(regs: &[ModelListenerReg], entity: &str) -> Vec<Arc<dyn ModelListener>> {
        regs.iter()
            .filter(|r| r.matches(entity))
            .map(|r| r.listener.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_listener_mutation_reaches_the_persister_and_the_id_comes_back() {
        let (db, _guard) = connect_test(&[]).await;
        let spy = SpyPersister::default();
        let log = Arc::new(Mutex::new(Vec::new()));
        let regs = vec![ModelListenerReg::all(Arc::new(Stamp {
            key: "created_at",
            log: log.clone(),
        }))];
        let mut rec = Record::new("samples");
        rec.set("name", "Chair");

        save(
            &db,
            &listeners(&regs, "samples"),
            &spy,
            &mut rec,
            Op::Create,
            None,
        )
        .await
        .unwrap();

        let seen = spy.seen.lock().unwrap();
        assert_eq!(seen[0].0, "create");
        assert_eq!(seen[0].1.get("created_at").map(String::as_str), Some("set"));
        assert_eq!(rec.id(), Some(42), "the create id lands on the record");
        assert_eq!(*log.lock().unwrap(), ["created_at"]);
    }

    #[tokio::test]
    async fn a_veto_stops_the_write_and_returns_field_messages() {
        let (db, _guard) = connect_test(&[]).await;
        let spy = SpyPersister::default();
        let regs = vec![ModelListenerReg::all(Arc::new(Veto))];
        let mut rec = Record::new("samples");

        let err = save(
            &db,
            &listeners(&regs, "samples"),
            &spy,
            &mut rec,
            Op::Create,
            None,
        )
        .await
        .expect_err("the veto refuses the write");

        match err {
            SaveError::Invalid(bag) => assert_eq!(bag.messages("name").len(), 1),
            SaveError::Failed(e) => panic!("expected a field error, got {e}"),
        }
        assert!(
            spy.seen.lock().unwrap().is_empty(),
            "nothing reached the persister"
        );
    }

    #[tokio::test]
    async fn listeners_run_in_registration_order() {
        let (db, _guard) = connect_test(&[]).await;
        let log = Arc::new(Mutex::new(Vec::new()));
        let regs = vec![
            ModelListenerReg::all(Arc::new(Stamp {
                key: "first",
                log: log.clone(),
            })),
            ModelListenerReg::all(Arc::new(Stamp {
                key: "second",
                log: log.clone(),
            })),
        ];
        let mut rec = Record::new("samples");

        save(
            &db,
            &listeners(&regs, "samples"),
            &SpyPersister::default(),
            &mut rec,
            Op::Create,
            None,
        )
        .await
        .unwrap();

        assert_eq!(*log.lock().unwrap(), ["first", "second"]);
    }

    #[tokio::test]
    async fn a_target_selects_which_listeners_run() {
        let (db, _guard) = connect_test(&[]).await;
        let log = Arc::new(Mutex::new(Vec::new()));
        let regs = vec![
            ModelListenerReg::all(Arc::new(Stamp {
                key: "everywhere",
                log: log.clone(),
            })),
            ModelListenerReg::for_entity(
                "orders",
                Arc::new(Stamp {
                    key: "orders_only",
                    log: log.clone(),
                }),
            ),
        ];
        let mut rec = Record::new("samples");

        save(
            &db,
            &listeners(&regs, "samples"),
            &SpyPersister::default(),
            &mut rec,
            Op::Create,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            *log.lock().unwrap(),
            ["everywhere"],
            "another entity's listener stays out"
        );
        assert!(matches!(regs[1].target, ListenerTarget::Entity(ref e) if e == "orders"));
    }

    #[tokio::test]
    async fn a_listener_injected_attribute_is_written_not_dropped() {
        let (db, _guard) = connect_test(&[]).await;
        let spy = SpyPersister::default();
        let log = Arc::new(Mutex::new(Vec::new()));
        let regs = vec![ModelListenerReg::all(Arc::new(Stamp {
            key: "created_at",
            log,
        }))];
        let mut rec = Record::new("samples");
        rec.set("name", "Chair");

        save(
            &db,
            &listeners(&regs, "samples"),
            &spy,
            &mut rec,
            Op::Create,
            None,
        )
        .await
        .unwrap();

        let default = DefaultPersister {
            entity: "samples".into(),
            id_field: "id".into(),
            columns: vec!["name".into()],
        };
        // The descriptor knows only `name`; the listener's key widens the write.
        assert_eq!(default.write_columns(&rec), ["name", "created_at"]);
    }

    #[test]
    fn the_id_column_is_never_written() {
        let default = DefaultPersister {
            entity: "samples".into(),
            id_field: "id".into(),
            columns: vec!["name".into()],
        };
        let mut rec = Record::with_id("samples", 3);
        rec.set("name", "Chair").set("id", 3i64);
        assert_eq!(default.write_columns(&rec), ["name"]);
    }

    #[test]
    fn binding_maps_each_kind_to_its_storage_form() {
        // Absent, null and empty text all bind NULL, so an optional non-text
        // column stores rather than failing on "".
        let null: SimpleExpr = Option::<String>::None.into();
        assert_eq!(bind(None), null);
        assert_eq!(bind(Some(&AttrValue::Null)), null);
        assert_eq!(bind(Some(&AttrValue::Text(String::new()))), null);
        assert_eq!(bind(Some(&AttrValue::Int(7))), 7i64.into());
        // Booleans store as integers, the representation every backend indexes.
        assert_eq!(bind(Some(&AttrValue::Bool(true))), 1i32.into());
        assert_eq!(
            bind(Some(&AttrValue::Text("Chair".into()))),
            Some("Chair".to_string()).into()
        );
    }

    #[tokio::test]
    async fn an_update_passes_its_id_through() {
        let (db, _guard) = connect_test(&[]).await;
        let spy = SpyPersister::default();
        let mut rec = Record::with_id("samples", 7);

        save(&db, &[], &spy, &mut rec, Op::Update, Some("7"))
            .await
            .unwrap();

        assert_eq!(spy.seen.lock().unwrap()[0].0, "update:7");
    }

    #[tokio::test]
    async fn an_update_without_an_id_fails_rather_than_writing() {
        let (db, _guard) = connect_test(&[]).await;
        let spy = SpyPersister::default();
        let mut rec = Record::new("samples");

        let err = save(&db, &[], &spy, &mut rec, Op::Update, None)
            .await
            .expect_err("an update needs an id");

        assert!(matches!(err, SaveError::Failed(_)));
        assert!(spy.seen.lock().unwrap().is_empty());
    }
}
