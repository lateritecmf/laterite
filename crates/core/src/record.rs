//! The record layer: a dynamic attribute bag and the lifecycle listeners that
//! shape a write.
//!
//! A [`Record`] is a map of attribute names to typed values, keyed by an entity
//! name rather than modelled as a compile-time struct. That is what lets one
//! pipeline serve both the fixed tables a module declares and content types
//! defined at runtime, and what lets a listener registered by one module reach
//! another module's entity.
//!
//! A record is a payload, not a live object: it has no `save`, no identity map
//! and no lazy loading. Code that wants a struct view converts at the edge with
//! [`Record::deserialize`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::validation::ErrorBag;
use crate::Db;

/// One attribute's value.
///
/// Non-exhaustive: variants are added as the storage layer learns to bind them,
/// so match with a `_` arm.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AttrValue {
    /// No value. Stored as SQL `NULL`, distinct from an empty string.
    Null,
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// An instant, always UTC. Rendered as RFC 3339 with microsecond precision,
    /// the one timestamp format the framework stores.
    DateTime(DateTime<Utc>),
    Json(Value),
}

impl AttrValue {
    /// Whether this is [`AttrValue::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, AttrValue::Null)
    }

    /// The storage text for this value, or `None` for [`AttrValue::Null`].
    ///
    /// This is the lossy bridge to the all-text write path; typed binding
    /// replaces it where the storage layer can bind a value natively.
    pub fn to_text(&self) -> Option<String> {
        match self {
            AttrValue::Null => None,
            AttrValue::Text(s) => Some(s.clone()),
            AttrValue::Int(i) => Some(i.to_string()),
            AttrValue::Float(f) => Some(f.to_string()),
            AttrValue::Bool(b) => Some(b.to_string()),
            AttrValue::DateTime(dt) => Some(dt.to_rfc3339_opts(SecondsFormat::Micros, true)),
            AttrValue::Json(v) => Some(v.to_string()),
        }
    }

    /// The text, when this is [`AttrValue::Text`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            AttrValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The integer, when this is [`AttrValue::Int`].
    pub fn as_int(&self) -> Option<i64> {
        match self {
            AttrValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The float, when this is [`AttrValue::Float`] (or an [`AttrValue::Int`],
    /// widened).
    pub fn as_float(&self) -> Option<f64> {
        match self {
            AttrValue::Float(f) => Some(*f),
            AttrValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// The boolean, when this is [`AttrValue::Bool`].
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            AttrValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The instant, when this is [`AttrValue::DateTime`].
    pub fn as_datetime(&self) -> Option<DateTime<Utc>> {
        match self {
            AttrValue::DateTime(dt) => Some(*dt),
            _ => None,
        }
    }

    /// The JSON, when this is [`AttrValue::Json`].
    pub fn as_json(&self) -> Option<&Value> {
        match self {
            AttrValue::Json(v) => Some(v),
            _ => None,
        }
    }

    /// This value as JSON, for [`Record::deserialize`]. Every variant maps to a
    /// natural JSON type; a datetime becomes its RFC 3339 string.
    fn to_json(&self) -> Value {
        match self {
            AttrValue::Null => Value::Null,
            AttrValue::Text(s) => Value::String(s.clone()),
            AttrValue::Int(i) => Value::from(*i),
            AttrValue::Float(f) => Value::from(*f),
            AttrValue::Bool(b) => Value::Bool(*b),
            AttrValue::DateTime(dt) => {
                Value::String(dt.to_rfc3339_opts(SecondsFormat::Micros, true))
            }
            AttrValue::Json(v) => v.clone(),
        }
    }
}

impl From<&str> for AttrValue {
    fn from(v: &str) -> Self {
        AttrValue::Text(v.to_string())
    }
}

impl From<String> for AttrValue {
    fn from(v: String) -> Self {
        AttrValue::Text(v)
    }
}

impl From<i64> for AttrValue {
    fn from(v: i64) -> Self {
        AttrValue::Int(v)
    }
}

impl From<f64> for AttrValue {
    fn from(v: f64) -> Self {
        AttrValue::Float(v)
    }
}

impl From<bool> for AttrValue {
    fn from(v: bool) -> Self {
        AttrValue::Bool(v)
    }
}

impl From<DateTime<Utc>> for AttrValue {
    fn from(v: DateTime<Utc>) -> Self {
        AttrValue::DateTime(v)
    }
}

impl From<Value> for AttrValue {
    fn from(v: Value) -> Self {
        AttrValue::Json(v)
    }
}

/// `None` becomes [`AttrValue::Null`], so an optional column reads naturally.
impl<T: Into<AttrValue>> From<Option<T>> for AttrValue {
    fn from(v: Option<T>) -> Self {
        v.map_or(AttrValue::Null, Into::into)
    }
}

/// Which stage of a write a listener is seeing.
///
/// Non-exhaustive: deletion joins when the delete path lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Op {
    Create,
    Update,
}

/// A dynamic record: an entity name, an optional id, and its attributes.
#[derive(Debug, Clone, Default)]
pub struct Record {
    entity: String,
    id: Option<i64>,
    attrs: BTreeMap<String, AttrValue>,
}

impl Record {
    /// An empty record for `entity`, with no id (the shape of a create).
    pub fn new(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            id: None,
            attrs: BTreeMap::new(),
        }
    }

    /// An empty record for an existing row (the shape of an update).
    pub fn with_id(entity: impl Into<String>, id: i64) -> Self {
        Self {
            entity: entity.into(),
            id: Some(id),
            attrs: BTreeMap::new(),
        }
    }

    /// The entity this record belongs to: a fixed table or a runtime content type.
    pub fn entity(&self) -> &str {
        &self.entity
    }

    /// The primary key, `None` until a create returns one.
    pub fn id(&self) -> Option<i64> {
        self.id
    }

    /// Records the id a create returned.
    pub fn set_id(&mut self, id: i64) {
        self.id = Some(id);
    }

    /// Sets an attribute, replacing any previous value. Chainable.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<AttrValue>) -> &mut Self {
        self.attrs.insert(key.into(), value.into());
        self
    }

    /// An attribute, or `None` when the key is absent. An attribute that is
    /// present and null returns `Some(AttrValue::Null)`, which is a different
    /// thing: absent means "not part of this write", null means "write NULL".
    pub fn get(&self, key: &str) -> Option<&AttrValue> {
        self.attrs.get(key)
    }

    /// Removes an attribute, returning it.
    pub fn remove(&mut self, key: &str) -> Option<AttrValue> {
        self.attrs.remove(key)
    }

    /// Whether the key is present, null or not.
    pub fn contains(&self, key: &str) -> bool {
        self.attrs.contains_key(key)
    }

    /// The attributes, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &AttrValue)> {
        self.attrs.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// How many attributes are set.
    pub fn len(&self) -> usize {
        self.attrs.len()
    }

    /// Whether no attribute is set.
    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }

    /// The text at `key`, when it holds [`AttrValue::Text`].
    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(AttrValue::as_str)
    }

    /// The integer at `key`.
    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(AttrValue::as_int)
    }

    /// The float at `key`.
    pub fn float(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(AttrValue::as_float)
    }

    /// The boolean at `key`.
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(AttrValue::as_bool)
    }

    /// The instant at `key`.
    pub fn datetime(&self, key: &str) -> Option<DateTime<Utc>> {
        self.get(key).and_then(AttrValue::as_datetime)
    }

    /// The JSON at `key`.
    pub fn json(&self, key: &str) -> Option<&Value> {
        self.get(key).and_then(AttrValue::as_json)
    }

    /// Reads the attributes into a struct, for code that would rather work with
    /// types than a bag. Attributes the target does not name are ignored.
    pub fn deserialize<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        let map = self
            .attrs
            .iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect::<serde_json::Map<_, _>>();
        serde_json::from_value(Value::Object(map))
    }

    /// Builds a record from an all-text submission, the shape the form write
    /// path uses today. Every value arrives as [`AttrValue::Text`], because a
    /// string carries no type; typed values come from field types and listeners.
    pub fn from_text_map(entity: impl Into<String>, data: &HashMap<String, String>) -> Self {
        let mut rec = Record::new(entity);
        for (k, v) in data {
            rec.set(k.clone(), v.clone());
        }
        rec
    }

    /// The record as an all-text map, for a storage layer that binds strings.
    /// A null attribute becomes an empty string, matching that path's existing
    /// "empty means none" contract.
    pub fn to_text_map(&self) -> HashMap<String, String> {
        self.attrs
            .iter()
            .map(|(k, v)| (k.clone(), v.to_text().unwrap_or_default()))
            .collect()
    }
}

/// Which entities a listener runs for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerTarget {
    /// One entity, by name.
    Entity(String),
    /// Every entity, the hook a cross-cutting concern (auditing, timestamps)
    /// attaches to.
    All,
}

impl ListenerTarget {
    /// Whether this target covers `entity`.
    pub fn matches(&self, entity: &str) -> bool {
        match self {
            ListenerTarget::Entity(name) => name == entity,
            ListenerTarget::All => true,
        }
    }
}

/// Shapes a write as it happens.
///
/// [`before_save`](ModelListener::before_save) runs outside the storage layer's
/// transaction and may change attributes or refuse the write.
/// [`after_save`](ModelListener::after_save) runs once the write succeeded and
/// is **not** atomic with it: work that must commit or roll back with the row
/// belongs in the storage layer, not here.
#[async_trait]
pub trait ModelListener: Send + Sync + 'static {
    /// Runs before the write. Returning an [`ErrorBag`] refuses it, and those
    /// messages reach the operator against their fields.
    async fn before_save(&self, db: &Db, rec: &mut Record, op: Op) -> Result<(), ErrorBag> {
        let _ = (db, rec, op);
        Ok(())
    }

    /// Runs after a successful write, for side effects.
    async fn after_save(&self, db: &Db, rec: &Record, op: Op) {
        let _ = (db, rec, op);
    }
}

/// A registered listener and the entities it runs for.
pub struct ModelListenerReg {
    pub target: ListenerTarget,
    pub listener: Arc<dyn ModelListener>,
}

impl ModelListenerReg {
    /// Registers a listener for one entity.
    pub fn for_entity(entity: impl Into<String>, listener: Arc<dyn ModelListener>) -> Self {
        Self {
            target: ListenerTarget::Entity(entity.into()),
            listener,
        }
    }

    /// Registers a listener for every entity.
    pub fn all(listener: Arc<dyn ModelListener>) -> Self {
        Self {
            target: ListenerTarget::All,
            listener,
        }
    }

    /// Whether this registration runs for `entity`.
    pub fn matches(&self, entity: &str) -> bool {
        self.target.matches(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde::Deserialize;

    fn instant() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 8, 12, 30, 45).unwrap()
    }

    #[test]
    fn attributes_round_trip_through_typed_accessors() {
        let mut rec = Record::new("products");
        rec.set("name", "Chair")
            .set("stock", 4i64)
            .set("price", 19.5f64)
            .set("active", true)
            .set("created_at", instant())
            .set("meta", serde_json::json!({"colour": "red"}));

        assert_eq!(rec.entity(), "products");
        assert_eq!(rec.id(), None);
        assert_eq!(rec.text("name"), Some("Chair"));
        assert_eq!(rec.int("stock"), Some(4));
        assert_eq!(rec.float("price"), Some(19.5));
        assert_eq!(rec.bool("active"), Some(true));
        assert_eq!(rec.datetime("created_at"), Some(instant()));
        assert_eq!(rec.json("meta").unwrap()["colour"], "red");
        assert_eq!(rec.len(), 6);
        // A typed read of the wrong kind is None, not a panic or a coercion.
        assert_eq!(rec.int("name"), None);
    }

    #[test]
    fn an_int_widens_to_float_but_nothing_else_coerces() {
        let mut rec = Record::new("products");
        rec.set("stock", 4i64);
        assert_eq!(rec.float("stock"), Some(4.0));
        assert_eq!(rec.bool("stock"), None);
        assert_eq!(rec.text("stock"), None);
    }

    #[test]
    fn absent_and_null_are_different() {
        let mut rec = Record::new("products");
        rec.set("note", None::<String>);
        assert!(rec.contains("note"));
        assert!(rec.get("note").unwrap().is_null());
        assert!(rec.get("missing").is_none());
        // Null carries no storage text; absent is not in the map at all.
        assert_eq!(AttrValue::Null.to_text(), None);
    }

    #[test]
    fn timestamps_render_in_the_one_stored_format() {
        assert_eq!(
            AttrValue::DateTime(instant()).to_text().unwrap(),
            "2026-09-08T12:30:45.000000Z"
        );
    }

    #[test]
    fn the_text_bridge_survives_a_round_trip() {
        let mut data = HashMap::new();
        data.insert("name".to_string(), "Chair".to_string());
        data.insert("note".to_string(), String::new());

        let rec = Record::from_text_map("products", &data);
        assert_eq!(rec.text("name"), Some("Chair"));
        // A string carries no type, so everything arrives as text.
        assert_eq!(rec.text("note"), Some(""));
        assert_eq!(rec.to_text_map(), data);
    }

    #[test]
    fn a_null_attribute_writes_empty_through_the_text_bridge() {
        let mut rec = Record::new("products");
        rec.set("name", "Chair").set("note", None::<String>);
        let map = rec.to_text_map();
        assert_eq!(map.get("note").map(String::as_str), Some(""));
    }

    #[test]
    fn typed_values_render_for_a_text_storage_layer() {
        let mut rec = Record::new("products");
        rec.set("stock", 4i64).set("active", false);
        let map = rec.to_text_map();
        assert_eq!(map.get("stock").map(String::as_str), Some("4"));
        assert_eq!(map.get("active").map(String::as_str), Some("false"));
    }

    #[test]
    fn deserialize_gives_a_struct_view_and_ignores_extra_attributes() {
        #[derive(Deserialize, PartialEq, Debug)]
        struct Product {
            name: String,
            stock: i64,
            active: bool,
        }

        let mut rec = Record::new("products");
        rec.set("name", "Chair")
            .set("stock", 4i64)
            .set("active", true)
            .set("internal_note", "ignore me");

        let product: Product = rec.deserialize().unwrap();
        assert_eq!(
            product,
            Product {
                name: "Chair".into(),
                stock: 4,
                active: true
            }
        );
    }

    #[test]
    fn an_id_is_absent_on_create_and_recorded_after() {
        let mut rec = Record::new("products");
        assert_eq!(rec.id(), None);
        rec.set_id(7);
        assert_eq!(rec.id(), Some(7));
        assert_eq!(Record::with_id("products", 3).id(), Some(3));
    }

    #[test]
    fn a_target_selects_the_entities_it_runs_for() {
        let one = ListenerTarget::Entity("products".into());
        assert!(one.matches("products"));
        assert!(!one.matches("orders"));
        assert!(ListenerTarget::All.matches("anything"));
    }

    #[test]
    fn a_registration_carries_its_target() {
        struct Noop;
        impl ModelListener for Noop {}

        let reg = ModelListenerReg::for_entity("products", Arc::new(Noop));
        assert!(reg.matches("products"));
        assert!(!reg.matches("orders"));
        assert!(ModelListenerReg::all(Arc::new(Noop)).matches("orders"));
    }

    #[test]
    fn attributes_iterate_in_key_order() {
        let mut rec = Record::new("products");
        rec.set("zebra", "z").set("alpha", "a");
        let keys: Vec<_> = rec.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["alpha", "zebra"]);
    }
}
