//! Events: facts a module announces, and the listeners that act on them.
//!
//! An event is a serde struct with a dot-keyed name. Whoever owns the fact
//! emits it once it is true; any module listens by registering a [`Listener`]
//! from its [`register`](crate::module::Module::register). Listeners run in
//! dependency order, one after another, inside the call that emitted.
//!
//! ```
//! use laterite_core::events::{Event, EventCx, EventError, Listener};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct Published {
//!     article_id: i64,
//! }
//!
//! impl Event for Published {
//!     const NAME: &'static str = "acme.blog.published";
//! }
//!
//! struct Announce;
//!
//! #[laterite_core::strata::async_trait]
//! impl Listener<Published> for Announce {
//!     async fn handle(&self, _cx: &EventCx<'_>, event: &Published) -> Result<(), EventError> {
//!         println!("article {} is out", event.article_id);
//!         Ok(())
//!     }
//! }
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

use crate::db::Db;
use crate::error::{CoreError, CoreResult};
use crate::migration::DbBackend;
use crate::module::ModuleId;
use crate::registry::Registry;

/// What a listener returns when it could not do its work. Any error converts
/// with `?`.
pub type EventError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A fact worth announcing: a serde struct carrying a dot-keyed name.
///
/// The name is past tense and starts with the owning module's id
/// (`acme.blog.published`), so two modules cannot claim one name by accident.
/// The payload is serde in both directions, so an event can leave the process.
pub trait Event: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The event's name, unique across the application.
    const NAME: &'static str;
}

/// Acts on one kind of event.
///
/// A listener runs after the fact it hears about, so it cannot refuse it. An
/// `Err` is logged with the event's name and the listening module, and the
/// listeners after it still run.
#[async_trait]
pub trait Listener<E: Event>: Send + Sync + 'static {
    async fn handle(&self, cx: &EventCx<'_>, event: &E) -> Result<(), EventError>;
}

/// What a listener works with besides the event.
///
/// Fields are private so more can be offered later without breaking a listener.
pub struct EventCx<'a> {
    db: &'a Db,
    events: &'a Events,
}

impl EventCx<'_> {
    /// The pool. `Db` is `Clone`, so a listener that spawns work clones it.
    pub fn db(&self) -> &Db {
        self.db
    }

    pub fn backend(&self) -> DbBackend {
        self.db.backend
    }

    /// The bus, for a listener that announces a fact of its own.
    pub fn events(&self) -> &Events {
        self.events
    }
}

/// A listener with its event type erased, so listeners for every event sit in
/// one collection.
#[async_trait]
trait Erased: Send + Sync {
    async fn handle(
        &self,
        cx: &EventCx<'_>,
        event: &(dyn Any + Send + Sync),
    ) -> Result<(), EventError>;
}

struct Typed<E, L> {
    listener: L,
    event: PhantomData<fn(E)>,
}

#[async_trait]
impl<E: Event, L: Listener<E>> Erased for Typed<E, L> {
    async fn handle(
        &self,
        cx: &EventCx<'_>,
        event: &(dyn Any + Send + Sync),
    ) -> Result<(), EventError> {
        match event.downcast_ref::<E>() {
            Some(event) => self.listener.handle(cx, event).await,
            // Listeners are stored under their event's type, so nothing else
            // is ever handed to one.
            None => Ok(()),
        }
    }
}

/// A listener as a module contributes it. [`Registry::listen`] builds one.
pub struct EventListenerReg {
    event: TypeId,
    name: &'static str,
    type_name: &'static str,
    listener: Arc<dyn Erased>,
}

impl EventListenerReg {
    /// Registers `listener` for the event `E`.
    pub fn on<E: Event>(listener: impl Listener<E>) -> Self {
        Self {
            event: TypeId::of::<E>(),
            name: E::NAME,
            type_name: std::any::type_name::<E>(),
            listener: Arc::new(Typed {
                listener,
                event: PhantomData::<fn(E)>,
            }),
        }
    }

    /// The name of the event this listener hears.
    pub fn event_name(&self) -> &'static str {
        self.name
    }
}

impl Registry {
    /// Contributes a listener for the event `E`.
    ///
    /// ```ignore
    /// fn register(&self, registry: &mut Registry) {
    ///     registry.listen::<Published>(Announce);
    /// }
    /// ```
    pub fn listen<E: Event>(&mut self, listener: impl Listener<E>) {
        self.add(EventListenerReg::on::<E>(listener));
    }
}

struct Bound {
    owner: Option<ModuleId>,
    listener: Arc<dyn Erased>,
}

struct Inner {
    db: Db,
    by_event: HashMap<TypeId, Vec<Bound>>,
}

/// The event bus. Cheap to clone: every clone is the same bus.
#[derive(Clone)]
pub struct Events {
    inner: Arc<Inner>,
}

impl Events {
    /// A bus nothing listens on. Emitting on it does nothing.
    pub fn new(db: Db) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                by_event: HashMap::new(),
            }),
        }
    }

    /// Starts a bus, for listeners added by hand: a test, or an application
    /// assembled without the bootstrap.
    pub fn builder(db: Db) -> EventsBuilder {
        EventsBuilder {
            db,
            listeners: Vec::new(),
        }
    }

    /// Announces `event` to its listeners, in the order they were registered,
    /// and returns once the last has run.
    ///
    /// Emit once the fact is true: after the transaction that made it so has
    /// committed. A listener that fails is logged and does not stop the others,
    /// and nothing reaches the caller, since the fact stands either way.
    pub async fn emit<E: Event>(&self, event: &E) {
        let Some(listeners) = self.inner.by_event.get(&TypeId::of::<E>()) else {
            tracing::debug!(event = E::NAME, listeners = 0, "event emitted");
            return;
        };
        tracing::debug!(
            event = E::NAME,
            listeners = listeners.len(),
            "event emitted"
        );
        let cx = EventCx {
            db: &self.inner.db,
            events: self,
        };
        for bound in listeners {
            if let Err(error) = bound.listener.handle(&cx, event).await {
                tracing::error!(
                    event = E::NAME,
                    module = bound.owner.map(|m| m.as_str()).unwrap_or("application"),
                    error = %error,
                    "an event listener failed"
                );
            }
        }
    }

    /// How many listeners hear the event `E`.
    pub fn listeners<E: Event>(&self) -> usize {
        self.inner
            .by_event
            .get(&TypeId::of::<E>())
            .map_or(0, Vec::len)
    }
}

/// Collects listeners, then builds the [`Events`] bus.
pub struct EventsBuilder {
    db: Db,
    listeners: Vec<(Option<ModuleId>, EventListenerReg)>,
}

impl EventsBuilder {
    /// Adds a listener for the event `E`.
    pub fn listen<E: Event>(mut self, listener: impl Listener<E>) -> Self {
        self.listeners
            .push((None, EventListenerReg::on::<E>(listener)));
        self
    }

    /// Adds a listener a module contributed, keeping the module's name for the
    /// log line a failure writes.
    pub fn registered(mut self, owner: ModuleId, listener: EventListenerReg) -> Self {
        self.listeners.push((Some(owner), listener));
        self
    }

    /// Builds the bus. Two event types under one name is refused, naming both.
    pub fn build(self) -> CoreResult<Events> {
        let mut named: HashMap<&'static str, (TypeId, &'static str)> = HashMap::new();
        let mut by_event: HashMap<TypeId, Vec<Bound>> = HashMap::new();
        for (owner, reg) in self.listeners {
            let (event, type_name) = *named.entry(reg.name).or_insert((reg.event, reg.type_name));
            if event != reg.event {
                return Err(CoreError::Config(format!(
                    "event name '{}' is declared by both {} and {}",
                    reg.name, type_name, reg.type_name
                )));
            }
            by_event.entry(reg.event).or_default().push(Bound {
                owner,
                listener: reg.listener,
            });
        }
        Ok(Events {
            inner: Arc::new(Inner {
                db: self.db,
                by_event,
            }),
        })
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::testing::connect_test;
    use serde::Deserialize;
    use std::sync::Mutex;

    #[derive(Serialize, Deserialize)]
    struct Published {
        article_id: i64,
    }

    impl Event for Published {
        const NAME: &'static str = "acme.blog.published";
    }

    #[derive(Serialize, Deserialize)]
    struct Archived {
        article_id: i64,
    }

    impl Event for Archived {
        const NAME: &'static str = "acme.blog.archived";
    }

    type Heard = Arc<Mutex<Vec<String>>>;

    /// Writes down what it heard, under its own name.
    struct Note(&'static str, Heard);

    #[async_trait]
    impl Listener<Published> for Note {
        async fn handle(&self, _cx: &EventCx<'_>, event: &Published) -> Result<(), EventError> {
            self.1
                .lock()
                .unwrap()
                .push(format!("{} published {}", self.0, event.article_id));
            Ok(())
        }
    }

    #[async_trait]
    impl Listener<Archived> for Note {
        async fn handle(&self, _cx: &EventCx<'_>, event: &Archived) -> Result<(), EventError> {
            self.1
                .lock()
                .unwrap()
                .push(format!("{} archived {}", self.0, event.article_id));
            Ok(())
        }
    }

    struct Fails;

    #[async_trait]
    impl Listener<Published> for Fails {
        async fn handle(&self, _cx: &EventCx<'_>, _event: &Published) -> Result<(), EventError> {
            Err("the mail server is away".into())
        }
    }

    /// Announces an archive for whatever was published, through the bus it was
    /// handed.
    struct ArchiveAtOnce;

    #[async_trait]
    impl Listener<Published> for ArchiveAtOnce {
        async fn handle(&self, cx: &EventCx<'_>, event: &Published) -> Result<(), EventError> {
            cx.events()
                .emit(&Archived {
                    article_id: event.article_id,
                })
                .await;
            Ok(())
        }
    }

    fn heard() -> Heard {
        Arc::new(Mutex::new(Vec::new()))
    }

    #[tokio::test]
    async fn listeners_hear_their_own_event_in_the_order_registered() {
        let (db, _guard) = connect_test(&[]).await;
        let log = heard();
        let events = Events::builder(db)
            .listen::<Published>(Note("first", log.clone()))
            .listen::<Archived>(Note("other", log.clone()))
            .listen::<Published>(Note("second", log.clone()))
            .build()
            .unwrap();

        events.emit(&Published { article_id: 7 }).await;

        assert_eq!(
            *log.lock().unwrap(),
            ["first published 7", "second published 7"]
        );
        assert_eq!(events.listeners::<Published>(), 2);
        assert_eq!(events.listeners::<Archived>(), 1);
    }

    #[tokio::test]
    async fn a_failing_listener_does_not_stop_the_next() {
        let (db, _guard) = connect_test(&[]).await;
        let log = heard();
        let events = Events::builder(db)
            .listen::<Published>(Fails)
            .listen::<Published>(Note("after", log.clone()))
            .build()
            .unwrap();

        events.emit(&Published { article_id: 3 }).await;

        assert_eq!(*log.lock().unwrap(), ["after published 3"]);
    }

    #[tokio::test]
    async fn a_listener_announces_a_fact_of_its_own() {
        let (db, _guard) = connect_test(&[]).await;
        let log = heard();
        let events = Events::builder(db)
            .listen::<Published>(ArchiveAtOnce)
            .listen::<Archived>(Note("chained", log.clone()))
            .build()
            .unwrap();

        events.emit(&Published { article_id: 9 }).await;

        assert_eq!(*log.lock().unwrap(), ["chained archived 9"]);
    }

    #[tokio::test]
    async fn an_event_nobody_hears_is_emitted_quietly() {
        let (db, _guard) = connect_test(&[]).await;
        let events = Events::new(db);
        events.emit(&Published { article_id: 1 }).await;
        assert_eq!(events.listeners::<Published>(), 0);
    }

    #[tokio::test]
    async fn one_name_for_two_events_is_refused() {
        #[derive(Serialize, Deserialize)]
        struct Imposter;
        impl Event for Imposter {
            const NAME: &'static str = "acme.blog.published";
        }
        struct Quiet;
        #[async_trait]
        impl Listener<Imposter> for Quiet {
            async fn handle(&self, _cx: &EventCx<'_>, _event: &Imposter) -> Result<(), EventError> {
                Ok(())
            }
        }

        let (db, _guard) = connect_test(&[]).await;
        let refused = Events::builder(db)
            .listen::<Published>(Note("first", heard()))
            .listen::<Imposter>(Quiet)
            .build();

        let Err(CoreError::Config(message)) = refused else {
            panic!("two types under one name must be refused");
        };
        assert!(message.contains("acme.blog.published"), "{message}");
        assert!(message.contains("Imposter"), "{message}");
    }

    #[tokio::test]
    async fn a_module_contributes_a_listener_through_the_registry() {
        let (db, _guard) = connect_test(&[]).await;
        let log = heard();
        let mut registry = Registry::new();
        registry.set_owner(ModuleId::new("acme.blog"));
        registry.listen::<Published>(Note("module", log.clone()));

        let mut builder = Events::builder(db);
        for (owner, reg) in registry.take_owned::<EventListenerReg>() {
            assert_eq!(reg.event_name(), "acme.blog.published");
            builder = builder.registered(owner, reg);
        }
        let events = builder.build().unwrap();
        events.emit(&Published { article_id: 4 }).await;

        assert_eq!(*log.lock().unwrap(), ["module published 4"]);
    }
}
