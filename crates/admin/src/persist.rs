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

use laterite_core::query::{bind_values, build as to_sql, insert_returning_id_on, text_cast};
use laterite_core::strata::async_trait;
use laterite_core::validation::ErrorBag;
use laterite_core::AnyRowExt;
use laterite_core::{t, Actor, AttrValue, Db, ModelListener, Op, Record, SaveCx, SavedCx, Text};
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

/// Why a delete failed.
#[derive(Debug)]
pub enum DeleteError {
    /// A listener or the persister refused it. Unlike a save, a delete has no
    /// fields to hang messages on, so this is one sentence for the operator,
    /// such as "three records still reference this one".
    Refused(Text),
    /// The delete itself failed: logged, and the operator sees a generic
    /// message.
    Failed(String),
}

/// Replaces the default insert/update for one form. An implementor owns its own
/// transaction when the write is multi-statement. `rec` holds the validated
/// submission plus anything the listeners added; `rec.to_text_map()` gives the
/// all-text shape if that suits an implementation better.
#[async_trait]
pub trait Persister: Send + Sync + 'static {
    /// Persists a new record, returning its id.
    async fn create(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<i64, SaveError>;
    /// Persists an edit to the record with primary key `id`.
    async fn update(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), SaveError>;

    /// The row as it stands, read in the same transaction, so listeners can see
    /// what an update changes. Returning `None` (the default) means this
    /// persister supplies no snapshot and change-based listeners sit out.
    async fn load(&self, cx: &mut SaveCx<'_>, id: &str) -> Result<Option<Record>, SaveError> {
        let _ = (cx, id);
        Ok(None)
    }

    /// Removes the record with primary key `id`, on the pipeline's transaction.
    ///
    /// The default refuses. A persister that writes through more than one table
    /// has to say how those rows come apart, and guessing at that is worse than
    /// declining: an implementor opts in by overriding this.
    async fn delete(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), DeleteError> {
        let _ = (cx, id, rec);
        Err(DeleteError::Refused(t!(
            "Deleting this kind of record is not supported."
        )))
    }
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
pub(crate) struct SaveRequest<'a> {
    pub db: &'a Db,
    pub listeners: &'a [Arc<dyn ModelListener>],
    pub persister: &'a dyn Persister,
    /// Who is performing the write; reaches listeners and the persister.
    pub actor: &'a Actor,
    pub op: Op,
    /// The row to update; `None` on a create.
    pub id: Option<&'a str>,
}

pub(crate) async fn save(req: SaveRequest<'_>, rec: &mut Record) -> Result<(), SaveError> {
    let SaveRequest {
        db,
        listeners,
        persister,
        actor,
        op,
        id,
    } = req;
    let mut tx = db
        .pool
        .begin()
        .await
        .map_err(|e| SaveError::Failed(e.to_string()))?;

    let outcome = save_in(
        &mut tx,
        db.backend,
        listeners,
        persister,
        actor.clone(),
        rec,
        op,
        id,
    )
    .await;
    match outcome {
        Ok(()) => tx
            .commit()
            .await
            .map_err(|e| SaveError::Failed(e.to_string()))?,
        Err(e) => {
            // A veto or a failed write rolls the whole save back, snapshot read
            // included.
            let _ = tx.rollback().await;
            return Err(e);
        }
    }

    // After the commit, so a listener here cannot roll the write back.
    let saved = SavedCx::new(db, actor);
    for listener in listeners {
        listener.after_save(&saved, rec, op).await;
    }
    Ok(())
}

/// What one delete needs: where to write, who is doing it, and which record.
pub(crate) struct DeleteRequest<'a> {
    pub db: &'a Db,
    pub listeners: &'a [Arc<dyn ModelListener>],
    pub persister: &'a dyn Persister,
    pub actor: &'a Actor,
    pub entity: &'a str,
    pub id: &'a str,
}

/// Removes one record through the same pipeline a save runs through: the
/// transaction is owned here, the listeners run inside it and may refuse, and
/// the after-stage runs once it has committed.
///
/// Returns the record as it was, so the caller can report what went.
pub(crate) async fn delete(req: DeleteRequest<'_>) -> Result<Record, DeleteError> {
    let DeleteRequest {
        db,
        listeners,
        persister,
        actor,
        entity,
        id,
    } = req;

    let mut tx = db
        .pool
        .begin()
        .await
        .map_err(|e| DeleteError::Failed(e.to_string()))?;

    let outcome = delete_in(
        &mut tx,
        db.backend,
        listeners,
        persister,
        actor.clone(),
        entity,
        id,
    )
    .await;
    let rec = match outcome {
        Ok(rec) => {
            tx.commit()
                .await
                .map_err(|e| DeleteError::Failed(e.to_string()))?;
            rec
        }
        Err(e) => {
            // A veto or a failed delete rolls the whole thing back, the read
            // included.
            let _ = tx.rollback().await;
            return Err(e);
        }
    };

    // After the commit: the row is gone, so nothing here can undo it.
    let saved = SavedCx::new(db, actor);
    for listener in listeners {
        listener.after_delete(&saved, &rec).await;
    }
    Ok(rec)
}

/// Everything a delete does inside the transaction: read the row, offer it to
/// the listeners, then remove it.
async fn delete_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: laterite_core::migration::DbBackend,
    listeners: &[Arc<dyn ModelListener>],
    persister: &dyn Persister,
    actor: Actor,
    entity: &str,
    id: &str,
) -> Result<Record, DeleteError> {
    let mut cx = SaveCx::new(backend, tx, actor);

    // The row as it stands, so a listener can decide on its contents and the
    // after-stage can report what was removed. A persister that supplies no
    // snapshot leaves an identity-only record, the same as on an update.
    let loaded = persister.load(&mut cx, id).await.map_err(|e| match e {
        SaveError::Invalid(_) => DeleteError::Failed("snapshot read refused".to_string()),
        SaveError::Failed(m) => DeleteError::Failed(m),
    })?;
    let mut rec = loaded.unwrap_or_else(|| Record::new(entity));
    if rec.id().is_none() {
        if let Ok(numeric) = id.parse::<i64>() {
            rec.set_id(numeric);
        }
    }

    for listener in listeners {
        listener
            .before_delete(&mut cx, &rec)
            .await
            .map_err(DeleteError::Refused)?;
    }

    persister.delete(&mut cx, id, &rec).await?;
    Ok(rec)
}

/// Everything a save does inside the transaction: read the pre-write row, run
/// the listeners, then write.
#[allow(clippy::too_many_arguments)]
async fn save_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: laterite_core::migration::DbBackend,
    listeners: &[Arc<dyn ModelListener>],
    persister: &dyn Persister,
    actor: Actor,
    rec: &mut Record,
    op: Op,
    id: Option<&str>,
) -> Result<(), SaveError> {
    let mut cx = SaveCx::new(backend, tx, actor);

    if op == Op::Update {
        if let Some(id) = id {
            if let Some(previous) = persister.load(&mut cx, id).await? {
                rec.set_original(previous);
            }
        }
    }

    for listener in listeners {
        listener
            .before_save(&mut cx, rec, op)
            .await
            .map_err(SaveError::Invalid)?;
    }

    match op {
        Op::Create => {
            let new_id = persister.create(&mut cx, rec).await?;
            rec.set_id(new_id);
        }
        Op::Update => {
            let id = id.ok_or_else(|| SaveError::Failed("update without an id".to_string()))?;
            persister.update(&mut cx, id, rec).await?;
        }
        // `Op` is non-exhaustive; a stage added later needs its arm here.
        _ => return Err(SaveError::Failed(format!("unsupported operation {op:?}"))),
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

    /// A persister over a list's own table, for a resource whose writes are not
    /// a generic form (a bespoke editor, or a read screen that still deletes).
    /// The list's columns become the snapshot a listener sees.
    pub(crate) fn from_list(config: &crate::list::ListConfig) -> Self {
        Self {
            entity: config.entity.clone(),
            id_field: config.id_field.clone(),
            columns: config.columns.iter().map(|c| c.field.clone()).collect(),
        }
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
    async fn create(&self, cx: &mut SaveCx<'_>, rec: &Record) -> Result<i64, SaveError> {
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
        let backend = cx.backend();
        insert_returning_id_on(backend, cx.conn(), stmt, Alias::new(&self.id_field))
            .await
            .map_err(|e| SaveError::Failed(e.to_string()))
    }

    async fn update(&self, cx: &mut SaveCx<'_>, id: &str, rec: &Record) -> Result<(), SaveError> {
        let backend = cx.backend();
        let (sql, values) = {
            let mut update = Query::update();
            update.table(Alias::new(&self.entity));
            for column in self.write_columns(rec) {
                update.value(Alias::new(&column), bind(rec.get(&column)));
            }
            update.and_where(
                Expr::col(Alias::new(&self.id_field))
                    .cast_as(Alias::new(text_cast(backend)))
                    .eq(id),
            );
            to_sql(backend, update)
        };
        bind_values(sqlx::query(&sql), values)
            .execute(cx.conn())
            .await
            .map(|_| ())
            .map_err(|e| SaveError::Failed(e.to_string()))
    }

    /// Removes the row. One table, one statement, on the pipeline's transaction,
    /// so a listener's veto rolls it back with everything else.
    async fn delete(
        &self,
        cx: &mut SaveCx<'_>,
        id: &str,
        _rec: &Record,
    ) -> Result<(), DeleteError> {
        let backend = cx.backend();
        let (sql, values) = {
            let mut del = Query::delete();
            del.from_table(Alias::new(&self.entity));
            del.and_where(
                Expr::col(Alias::new(&self.id_field))
                    .cast_as(Alias::new(text_cast(backend)))
                    .eq(id),
            );
            to_sql(backend, del)
        };
        bind_values(sqlx::query(&sql), values)
            .execute(cx.conn())
            .await
            .map(|_| ())
            .map_err(|e| DeleteError::Failed(e.to_string()))
    }

    /// Reads the row's declared columns in the transaction, so a listener sees
    /// what an update changes. Values come back as text, the shape `sqlx::Any`
    /// gives for a mixed row.
    async fn load(&self, cx: &mut SaveCx<'_>, id: &str) -> Result<Option<Record>, SaveError> {
        let backend = cx.backend();
        let (sql, values) = {
            let mut select = Query::select();
            select.from(Alias::new(&self.entity));
            for column in &self.columns {
                select.expr_as(
                    Expr::col(Alias::new(column)).cast_as(Alias::new(text_cast(backend))),
                    Alias::new(column),
                );
            }
            select.and_where(
                Expr::col(Alias::new(&self.id_field))
                    .cast_as(Alias::new(text_cast(backend)))
                    .eq(id),
            );
            to_sql(backend, select)
        };
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(cx.conn())
            .await
            .map_err(|e| SaveError::Failed(e.to_string()))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut previous = Record::new(&self.entity);
        for column in &self.columns {
            match row.get_text_opt(column) {
                Ok(Some(v)) => {
                    previous.set(column.clone(), v);
                }
                Ok(None) => {
                    previous.set(column.clone(), AttrValue::Null);
                }
                Err(e) => return Err(SaveError::Failed(e.to_string())),
            }
        }
        Ok(Some(previous))
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
        async fn create(&self, _cx: &mut SaveCx<'_>, rec: &Record) -> Result<i64, SaveError> {
            self.seen
                .lock()
                .unwrap()
                .push(("create".into(), rec.to_text_map()));
            Ok(42)
        }
        async fn update(
            &self,
            _cx: &mut SaveCx<'_>,
            id: &str,
            rec: &Record,
        ) -> Result<(), SaveError> {
            self.seen
                .lock()
                .unwrap()
                .push((format!("update:{id}"), rec.to_text_map()));
            Ok(())
        }
    }

    /// Records deletes, and can refuse them.
    #[derive(Default)]
    struct DeleteSpy {
        deleted: Mutex<Vec<String>>,
        refuse: Option<&'static str>,
    }

    #[async_trait]
    impl Persister for DeleteSpy {
        async fn create(&self, _cx: &mut SaveCx<'_>, _rec: &Record) -> Result<i64, SaveError> {
            Ok(1)
        }
        async fn update(
            &self,
            _cx: &mut SaveCx<'_>,
            _id: &str,
            _rec: &Record,
        ) -> Result<(), SaveError> {
            Ok(())
        }
        async fn load(&self, _cx: &mut SaveCx<'_>, id: &str) -> Result<Option<Record>, SaveError> {
            let mut rec = Record::new("samples");
            rec.set("code", format!("code-{id}"));
            if let Ok(n) = id.parse::<i64>() {
                rec.set_id(n);
            }
            Ok(Some(rec))
        }
        async fn delete(
            &self,
            _cx: &mut SaveCx<'_>,
            id: &str,
            _rec: &Record,
        ) -> Result<(), DeleteError> {
            if let Some(why) = self.refuse {
                return Err(DeleteError::Refused(Text::new(why)));
            }
            self.deleted.lock().unwrap().push(id.to_string());
            Ok(())
        }
    }

    /// Refuses every delete, the way a listener guarding its references would.
    struct Guard;

    #[async_trait]
    impl ModelListener for Guard {
        async fn before_delete(&self, _cx: &mut SaveCx<'_>, _rec: &Record) -> Result<(), Text> {
            Err(Text::new("still referenced"))
        }
    }

    /// Notes what the after-stage saw, which is the record as it was.
    #[derive(Default)]
    struct Mourner {
        seen: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ModelListener for Mourner {
        async fn after_delete(&self, _cx: &SavedCx<'_>, rec: &Record) {
            self.seen
                .lock()
                .unwrap()
                .push(rec.text("code").unwrap_or_default().to_string());
        }
    }

    #[tokio::test]
    async fn a_delete_runs_the_listeners_around_the_persister() {
        let (db, _guard) = connect_test(&[]).await;
        let persister = DeleteSpy::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let listeners: Vec<Arc<dyn ModelListener>> = vec![Arc::new(Mourner { seen: seen.clone() })];

        let rec = delete(DeleteRequest {
            db: &db,
            listeners: &listeners,
            persister: &persister,
            actor: &actor(),
            entity: "samples",
            id: "7",
        })
        .await
        .expect("deleted");

        assert_eq!(persister.deleted.lock().unwrap().as_slice(), ["7"]);
        // The after-stage sees the row as it was, which is the point of reading
        // it before the delete.
        assert_eq!(seen.lock().unwrap().as_slice(), ["code-7"]);
        assert_eq!(rec.text("code"), Some("code-7"));
        assert_eq!(rec.id(), Some(7));
    }

    /// A listener's refusal stops the delete: the persister is never reached and
    /// the after-stage never runs.
    #[tokio::test]
    async fn a_listener_can_refuse_a_delete() {
        let (db, _guard) = connect_test(&[]).await;
        let persister = DeleteSpy::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let listeners: Vec<Arc<dyn ModelListener>> =
            vec![Arc::new(Guard), Arc::new(Mourner { seen: seen.clone() })];

        let err = delete(DeleteRequest {
            db: &db,
            listeners: &listeners,
            persister: &persister,
            actor: &actor(),
            entity: "samples",
            id: "7",
        })
        .await
        .expect_err("refused");

        match err {
            DeleteError::Refused(message) => assert_eq!(message.source(), "still referenced"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(
            persister.deleted.lock().unwrap().is_empty(),
            "nothing removed"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "no after-stage on a refusal"
        );
    }

    /// A persister with no delete of its own says so rather than reporting
    /// success, since a silent no-op would look like the record went.
    #[tokio::test]
    async fn a_persister_without_delete_refuses() {
        let (db, _guard) = connect_test(&[]).await;
        let persister = SpyPersister::default();
        let err = delete(DeleteRequest {
            db: &db,
            listeners: &[],
            persister: &persister,
            actor: &actor(),
            entity: "samples",
            id: "1",
        })
        .await
        .expect_err("unsupported");
        assert!(matches!(err, DeleteError::Refused(_)));
    }

    /// Sets an attribute before the write, and appends its name to a shared log
    /// after it.
    struct Stamp {
        key: &'static str,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl ModelListener for Stamp {
        async fn before_save(
            &self,
            _cx: &mut SaveCx<'_>,
            rec: &mut Record,
            _op: Op,
        ) -> Result<(), ErrorBag> {
            rec.set(self.key, "set");
            Ok(())
        }
        async fn after_save(&self, _cx: &SavedCx<'_>, _rec: &Record, _op: Op) {
            self.log.lock().unwrap().push(self.key);
        }
    }

    /// Refuses every write.
    struct Veto;

    #[async_trait]
    impl ModelListener for Veto {
        async fn before_save(
            &self,
            _cx: &mut SaveCx<'_>,
            _rec: &mut Record,
            _op: Op,
        ) -> Result<(), ErrorBag> {
            let mut bag = ErrorBag::default();
            // A plain Text, not `t!`: a test fixture must not enter the catalog.
            bag.add("name", Text::new("Not allowed"));
            Err(bag)
        }
    }

    fn actor() -> Actor {
        Actor::system("test")
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
            SaveRequest {
                db: &db,
                listeners: &listeners(&regs, "samples"),
                persister: &spy,
                actor: &actor(),
                op: Op::Create,
                id: None,
            },
            &mut rec,
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
            SaveRequest {
                db: &db,
                listeners: &listeners(&regs, "samples"),
                persister: &spy,
                actor: &actor(),
                op: Op::Create,
                id: None,
            },
            &mut rec,
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
            SaveRequest {
                db: &db,
                listeners: &listeners(&regs, "samples"),
                persister: &SpyPersister::default(),
                actor: &actor(),
                op: Op::Create,
                id: None,
            },
            &mut rec,
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
            SaveRequest {
                db: &db,
                listeners: &listeners(&regs, "samples"),
                persister: &SpyPersister::default(),
                actor: &actor(),
                op: Op::Create,
                id: None,
            },
            &mut rec,
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
            SaveRequest {
                db: &db,
                listeners: &listeners(&regs, "samples"),
                persister: &spy,
                actor: &actor(),
                op: Op::Create,
                id: None,
            },
            &mut rec,
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

    /// Supplies a pre-write snapshot, as the built-in persister does.
    struct SnapshotPersister;

    #[async_trait]
    impl Persister for SnapshotPersister {
        async fn create(&self, _cx: &mut SaveCx<'_>, _rec: &Record) -> Result<i64, SaveError> {
            Ok(1)
        }
        async fn update(
            &self,
            _cx: &mut SaveCx<'_>,
            _id: &str,
            _rec: &Record,
        ) -> Result<(), SaveError> {
            Ok(())
        }
        async fn load(&self, _cx: &mut SaveCx<'_>, _id: &str) -> Result<Option<Record>, SaveError> {
            let mut previous = Record::new("samples");
            previous.set("name", "Chair");
            Ok(Some(previous))
        }
    }

    /// Reads the snapshot in `before_save` and records what it saw.
    struct WatchChanges {
        seen: Arc<Mutex<Vec<(bool, bool)>>>,
    }

    #[async_trait]
    impl ModelListener for WatchChanges {
        async fn before_save(
            &self,
            _cx: &mut SaveCx<'_>,
            rec: &mut Record,
            _op: Op,
        ) -> Result<(), ErrorBag> {
            self.seen
                .lock()
                .unwrap()
                .push((rec.has_original(), rec.changed("name")));
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_update_hands_listeners_the_pre_write_row() {
        let (db, _guard) = connect_test(&[]).await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let regs = vec![ModelListenerReg::all(Arc::new(WatchChanges {
            seen: seen.clone(),
        }))];
        let mut rec = Record::with_id("samples", 1);
        rec.set("name", "Stool");

        save(
            SaveRequest {
                db: &db,
                listeners: &listeners(&regs, "samples"),
                persister: &SnapshotPersister,
                actor: &actor(),
                op: Op::Update,
                id: Some("1"),
            },
            &mut rec,
        )
        .await
        .unwrap();

        assert_eq!(*seen.lock().unwrap(), [(true, true)]);
    }

    #[tokio::test]
    async fn a_create_has_no_snapshot() {
        let (db, _guard) = connect_test(&[]).await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let regs = vec![ModelListenerReg::all(Arc::new(WatchChanges {
            seen: seen.clone(),
        }))];
        let mut rec = Record::new("samples");
        rec.set("name", "Chair");

        save(
            SaveRequest {
                db: &db,
                listeners: &listeners(&regs, "samples"),
                persister: &SnapshotPersister,
                actor: &actor(),
                op: Op::Create,
                id: None,
            },
            &mut rec,
        )
        .await
        .unwrap();

        assert_eq!(*seen.lock().unwrap(), [(false, false)]);
    }

    #[tokio::test]
    async fn an_update_passes_its_id_through() {
        let (db, _guard) = connect_test(&[]).await;
        let spy = SpyPersister::default();
        let mut rec = Record::with_id("samples", 7);

        save(
            SaveRequest {
                db: &db,
                listeners: &[],
                persister: &spy,
                actor: &actor(),
                op: Op::Update,
                id: Some("7"),
            },
            &mut rec,
        )
        .await
        .unwrap();

        assert_eq!(spy.seen.lock().unwrap()[0].0, "update:7");
    }

    #[tokio::test]
    async fn an_update_without_an_id_fails_rather_than_writing() {
        let (db, _guard) = connect_test(&[]).await;
        let spy = SpyPersister::default();
        let mut rec = Record::new("samples");

        let err = save(
            SaveRequest {
                db: &db,
                listeners: &[],
                persister: &spy,
                actor: &actor(),
                op: Op::Update,
                id: None,
            },
            &mut rec,
        )
        .await
        .expect_err("an update needs an id");

        assert!(matches!(err, SaveError::Failed(_)));
        assert!(spy.seen.lock().unwrap().is_empty());
    }
}
