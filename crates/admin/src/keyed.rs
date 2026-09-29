//! Reading a YAML map of named things into an ordered `Vec`.
//!
//! A descriptor file names its columns, fields and filters:
//!
//! ```yaml
//! columns:
//!   title:      { searchable: true }
//!   created_at: { type: datetime }
//! ```
//!
//! The struct behind each is a `Vec` whose element carries the name, because
//! the handlers index by position and the augmentation seam inserts at one. The
//! adapter here bridges the two, and the element's own name field is never
//! written in the file: the map key is the name, so the two cannot disagree.
//!
//! Every serde YAML deserializer hands a map to `MapAccess` in document order,
//! so the order written is the order rendered. A duplicate key is refused by
//! the parser before this sees it.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

/// A descriptor element whose name comes from the key it was written under.
pub(crate) trait Keyed {
    fn key(&self) -> &str;
    fn set_key(&mut self, key: String);
}

/// Reads `name: { .. }` pairs into a `Vec`, each element told its own name.
pub(crate) fn deserialize<'de, D, T>(de: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Keyed,
{
    struct KeyedVisitor<T>(PhantomData<T>);

    impl<'de, T> Visitor<'de> for KeyedVisitor<T>
    where
        T: Deserialize<'de> + Keyed,
    {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a map of named entries")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::with_capacity(map.size_hint().unwrap_or(0));
            while let Some(key) = map.next_key::<String>()? {
                let mut value: T = map.next_value()?;
                value.set_key(key);
                out.push(value);
            }
            Ok(out)
        }

        /// An empty map arrives as a null when the key is written bare.
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }

    de.deserialize_map(KeyedVisitor(PhantomData))
}

/// Writes the `Vec` back as the map it was read from, so a round trip through
/// the file format returns what went in.
pub(crate) fn serialize<S, T>(items: &[T], ser: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize + Keyed,
{
    let mut map = ser.serialize_map(Some(items.len()))?;
    for item in items {
        map.serialize_entry(item.key(), item)?;
    }
    map.end()
}

/// Reads one entry of a descriptor map, keeping the keys the descriptor owns
/// apart from the keys that belong to the entry's type.
///
/// `own` is offered every key in turn: for one the descriptor owns it reads the
/// value and answers `true`. Any other key is the type's, kept as written, and
/// the type checks it when the screen is prepared, so `rows: 6` is valid on a
/// `textarea` and refused on a `switch`. Returns the type's keys as an object,
/// or null when there are none.
pub(crate) fn split_entry<'de, A, F>(mut map: A, mut own: F) -> Result<serde_json::Value, A::Error>
where
    A: MapAccess<'de>,
    F: FnMut(&str, &mut A) -> Result<bool, A::Error>,
{
    let mut theirs = serde_json::Map::new();
    while let Some(key) = map.next_key::<String>()? {
        if !own(&key, &mut map)? {
            let value: serde_json::Value = map.next_value()?;
            theirs.insert(key, value);
        }
    }
    // A type's choices are a list; a map under `options` is the nested bag a
    // type's keys used to be written in.
    if matches!(theirs.get("options"), Some(serde_json::Value::Object(_))) {
        return Err(de::Error::custom(
            "`options: { .. }` no longer nests a type's keys: write them on the entry itself",
        ));
    }
    Ok(if theirs.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(theirs)
    })
}

/// Writes a type's keys back beside the descriptor's own, the shape they are
/// read in.
pub(crate) fn serialize_theirs<M: SerializeMap>(
    map: &mut M,
    options: &serde_json::Value,
) -> Result<(), M::Error> {
    match options {
        serde_json::Value::Object(entries) => {
            for (key, value) in entries {
                map.serialize_entry(key, value)?;
            }
        }
        serde_json::Value::Null => {}
        other => map.serialize_entry("options", other)?,
    }
    Ok(())
}
