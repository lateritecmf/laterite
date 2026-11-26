//! Option sources: the choices a field offers, worked out when the form is
//! shown.
//!
//! A select, radio or checklist names a registered [`OptionSource`] in place
//! of a list (`options: acme.states`). The source sees the form's current
//! values, so one field's choices can follow another's: a field that
//! `depends_on` a second is drawn again, with fresh choices, when the second
//! changes.

use std::collections::HashMap;
use std::sync::Arc;

use laterite_core::strata::async_trait;
use laterite_core::{Db, Translator};
use serde::{Deserialize, Serialize};

/// One choice a field offers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OptionEntry {
    /// The value stored when the choice is taken.
    pub value: String,
    /// The text shown for it.
    pub label: String,
    /// A second line under the label, where the control has room for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Shown, and not selectable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// The heading the choice sits under. Choices naming the same group are
    /// shown together, in the order the first of them came.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl OptionEntry {
    /// A selectable choice storing `value`, shown as `label`.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            ..Self::default()
        }
    }

    /// Adds a second line under the label.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Shows the choice without letting it be taken.
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }

    /// Places the choice under a heading.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// What a source knows about the field it is filling.
#[non_exhaustive]
pub struct OptionCx<'a> {
    /// The field's name.
    pub field: &'a str,
    /// The field's current value, as its control holds it.
    pub value: &'a str,
    /// Every value the form or the filter bar holds, by field name.
    pub values: &'a HashMap<String, String>,
    /// The record being edited, when there is one.
    pub id: Option<&'a str>,
    /// The request's translator, for labels the source words itself.
    pub i18n: &'a Translator,
}

/// A source failed to produce its choices.
#[derive(Debug, thiserror::Error)]
#[error("option source error: {0}")]
pub struct OptionError(pub String);

/// A source of choices. Implementors build SQL in a scoped block and drop the
/// builder before awaiting, as a picker source does.
#[async_trait]
pub trait OptionSource: Send + Sync + 'static {
    /// The choices to offer, in the order to show them.
    async fn options(&self, db: &Db, cx: &OptionCx<'_>) -> Result<Vec<OptionEntry>, OptionError>;
}

/// A registered source under its dotted `vendor.name` key.
pub struct OptionSourceReg {
    /// The source's dotted `vendor.name` key.
    pub name: String,
    /// The source being registered.
    pub source: Arc<dyn OptionSource>,
}

impl OptionSourceReg {
    /// Registers `source` as `name`.
    pub fn new(name: impl Into<String>, source: Arc<dyn OptionSource>) -> Self {
        Self {
            name: name.into(),
            source,
        }
    }

    /// Registers a list that never changes as `name`.
    pub fn fixed(name: impl Into<String>, entries: Vec<OptionEntry>) -> Self {
        Self::new(name, Arc::new(Fixed(entries)))
    }
}

/// The option-source registry, keyed by source name.
pub type OptionRegistry = HashMap<String, OptionSourceReg>;

/// A source over a list fixed at registration.
struct Fixed(Vec<OptionEntry>);

#[async_trait]
impl OptionSource for Fixed {
    async fn options(&self, _db: &Db, _cx: &OptionCx<'_>) -> Result<Vec<OptionEntry>, OptionError> {
        Ok(self.0.clone())
    }
}

/// Where a field's choices come from, as its descriptor wrote `options`: a
/// list in place, or the name of a registered source.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Declared<T> {
    /// `options: vendor.name`.
    Source(String),
    /// `options: [..]`.
    List(Vec<T>),
}

impl<T> Default for Declared<T> {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

impl<T> Declared<T> {
    /// The declared list, empty when a source supplies it.
    pub(crate) fn list(&self) -> &[T] {
        match self {
            Self::List(list) => list,
            Self::Source(_) => &[],
        }
    }

    /// The source named, when one is.
    pub(crate) fn source(&self) -> Option<&str> {
        match self {
            Self::Source(name) => Some(name),
            Self::List(_) => None,
        }
    }
}

/// Entries gathered under their headings: each run is a heading (or none) and
/// the choices beneath it, in first-seen order.
pub(crate) fn grouped(entries: &[OptionEntry]) -> Vec<(Option<&str>, Vec<&OptionEntry>)> {
    let mut out: Vec<(Option<&str>, Vec<&OptionEntry>)> = Vec::new();
    for entry in entries {
        let group = entry.group.as_deref();
        match out.iter_mut().find(|(heading, _)| *heading == group) {
            Some((_, members)) => members.push(entry),
            None => out.push((group, vec![entry])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_keep_first_seen_order() {
        let entries = vec![
            OptionEntry::new("a", "A").group("One"),
            OptionEntry::new("b", "B"),
            OptionEntry::new("c", "C").group("One"),
        ];
        let runs = grouped(&entries);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, Some("One"));
        assert_eq!(runs[0].1.len(), 2);
        assert_eq!(runs[1].0, None);
    }

    #[test]
    fn options_read_as_a_list_or_a_source() {
        let source: Declared<OptionEntry> = serde_json::from_str(r#""acme.states""#).unwrap();
        assert_eq!(source.source(), Some("acme.states"));
        let list: Declared<OptionEntry> =
            serde_json::from_str(r#"[{"value":"a","label":"A"}]"#).unwrap();
        assert_eq!(list.list().len(), 1);
    }
}
