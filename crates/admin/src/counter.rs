//! Menu counters: the number beside a menu entry or a settings item.
//!
//! An entry names a registered [`Counter`] (`counter: acme.pending`). The
//! number is worked out for each page, for the operator looking at it, and
//! shown when it is above zero. A section with no counter of its own shows
//! the sum of its entries'.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use laterite_auth::PermissionSet;
use laterite_core::strata::async_trait;
use laterite_core::{Db, Text, Translator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::nav::NavNode;
use crate::settings::SettingsItem;

/// How long one counter may take before its entry shows no number.
const PATIENCE: Duration = Duration::from_millis(1500);

/// The largest number shown as it is; above it the entry shows `99+`.
const SHOWN_UP_TO: u64 = 99;

/// What a counter knows about who is looking.
#[non_exhaustive]
pub struct CounterCx<'a> {
    /// The signed-in user's id.
    pub user_id: i64,
    /// What the user may do, for a count that depends on it.
    pub permissions: &'a PermissionSet,
}

/// A counter failed to produce its number.
#[derive(Debug, thiserror::Error)]
#[error("counter error: {0}")]
pub struct CounterError(pub String);

/// A number shown beside a menu entry. Implementors build SQL in a scoped
/// block and drop the builder before awaiting, as an option source does.
#[async_trait]
pub trait Counter: Send + Sync + 'static {
    /// The number to show; zero shows nothing.
    async fn count(&self, db: &Db, cx: &CounterCx<'_>) -> Result<u64, CounterError>;
}

/// A registered counter under its dotted `vendor.name` key.
pub struct CounterReg {
    /// The counter's dotted `vendor.name` key.
    pub name: String,
    /// The counter being registered.
    pub counter: Arc<dyn Counter>,
}

impl CounterReg {
    /// Registers `counter` as `name`.
    pub fn new(name: impl Into<String>, counter: Arc<dyn Counter>) -> Self {
        Self {
            name: name.into(),
            counter,
        }
    }
}

/// The counter registry, keyed by counter name.
pub(crate) type CounterRegistry = HashMap<String, CounterReg>;

/// The counter an entry shows, as its `counter` key reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum CounterRef {
    /// Unset: an entry shows no number, a section the sum of its entries'.
    #[default]
    Inherit,
    /// `counter: false`: no number, a section's sum included.
    Off,
    /// `counter: vendor.name`: the registered counter of that name.
    Source(String),
}

impl CounterRef {
    /// The counter named, when one is.
    pub fn source(&self) -> Option<&str> {
        match self {
            Self::Source(name) => Some(name),
            _ => None,
        }
    }

    /// Whether nothing was set.
    pub fn is_inherit(&self) -> bool {
        matches!(self, Self::Inherit)
    }
}

impl From<&str> for CounterRef {
    fn from(name: &str) -> Self {
        Self::Source(name.to_owned())
    }
}

impl From<String> for CounterRef {
    fn from(name: String) -> Self {
        Self::Source(name)
    }
}

impl Serialize for CounterRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Inherit => serializer.serialize_none(),
            Self::Off => serializer.serialize_bool(false),
            Self::Source(name) => serializer.serialize_str(name),
        }
    }
}

impl<'de> Deserialize<'de> for CounterRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Key {
            Flag(bool),
            Name(String),
        }
        Ok(match Option::<Key>::deserialize(deserializer)? {
            Some(Key::Flag(false)) => Self::Off,
            Some(Key::Flag(true)) | None => Self::Inherit,
            Some(Key::Name(name)) => Self::Source(name),
        })
    }
}

/// The number beside one entry, as a page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Badge {
    /// The number; zero shows nothing.
    pub(crate) count: u64,
    /// What the number counts, localized, for the tooltip.
    pub(crate) label: String,
}

impl Badge {
    /// The number as the entry shows it.
    pub(crate) fn text(&self) -> String {
        text(self.count)
    }
}

/// A count as an entry shows it: the number, or `99+` above ninety-nine.
pub(crate) fn text(count: u64) -> String {
    if count > SHOWN_UP_TO {
        format!("{SHOWN_UP_TO}+")
    } else {
        count.to_string()
    }
}

/// Every entry's badge on one page, by the entry's key: a menu key, or a
/// settings item's code.
pub(crate) type Badges = HashMap<String, Badge>;

/// Runs each named counter once, together. One that fails, runs long or was
/// never registered counts as zero and is logged.
async fn run(
    registry: &CounterRegistry,
    names: BTreeSet<&str>,
    db: &Db,
    cx: &CounterCx<'_>,
) -> HashMap<String, u64> {
    let runs = names.into_iter().map(|name| async move {
        let Some(reg) = registry.get(name) else {
            return (name.to_owned(), 0);
        };
        let count = match tokio::time::timeout(PATIENCE, reg.counter.count(db, cx)).await {
            Ok(Ok(count)) => count,
            Ok(Err(e)) => {
                tracing::warn!(counter = %name, error = %e, "a menu counter failed");
                0
            }
            Err(_) => {
                tracing::warn!(counter = %name, "a menu counter ran too long");
                0
            }
        };
        (name.to_owned(), count)
    });
    futures::future::join_all(runs).await.into_iter().collect()
}

/// The badges for the menu and the settings items one operator sees.
///
/// `nav` and `settings` are already filtered to what the operator may open,
/// so nothing they cannot see is counted. The Settings tab shows the sum of
/// the settings items'.
pub(crate) async fn badges(
    registry: &CounterRegistry,
    nav: &[NavNode],
    settings: &[SettingsItem],
    db: &Db,
    cx: &CounterCx<'_>,
    tr: &Translator,
) -> Badges {
    let allowed = |p: &Option<String>| p.as_deref().is_none_or(|p| cx.permissions.allows(p));
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for node in nav {
        names.extend(node.item.counter.source());
        names.extend(node.children.iter().filter_map(|c| c.item.counter.source()));
    }
    for item in settings {
        names.extend(item.counter.source());
        names.extend(
            item.tabs
                .iter()
                .filter(|t| allowed(&t.permission))
                .filter_map(|t| item.tab_counters.get(&t.key))
                .filter_map(|(counter, _)| counter.source()),
        );
    }
    if names.is_empty() {
        return Badges::new();
    }
    let counts = run(registry, names, db, cx).await;
    let of = |counter: &CounterRef| counter.source().and_then(|n| counts.get(n)).copied();
    let label = |text: &Option<Text>| text.as_ref().map(|t| tr.t(t)).unwrap_or_default();
    let mut out = Badges::new();
    let mut settings_total = 0;
    for item in settings {
        let mut sum = 0;
        for tab in item.tabs.iter().filter(|t| allowed(&t.permission)) {
            let Some((counter, text)) = item.tab_counters.get(&tab.key) else {
                continue;
            };
            let count = of(counter).unwrap_or(0);
            sum += count;
            out.insert(
                tab.key.clone(),
                Badge {
                    count,
                    label: label(text),
                },
            );
        }
        let count = match &item.counter {
            CounterRef::Off => 0,
            CounterRef::Inherit => sum,
            counter => of(counter).unwrap_or(0),
        };
        settings_total += count;
        out.insert(
            item.code.clone(),
            Badge {
                count,
                label: label(&item.counter_label),
            },
        );
    }
    for node in nav {
        let mut sum = 0;
        for child in &node.children {
            let count = of(&child.item.counter).unwrap_or(0);
            sum += count;
            out.insert(
                child.item.key.clone(),
                Badge {
                    count,
                    label: label(&child.item.counter_label),
                },
            );
        }
        if node.item.key == "settings" && node.item.counter.is_inherit() {
            sum += settings_total;
        }
        let count = match &node.item.counter {
            CounterRef::Off => 0,
            CounterRef::Inherit => sum,
            counter => of(counter).unwrap_or(0),
        };
        out.insert(
            node.item.key.clone(),
            Badge {
                count,
                label: label(&node.item.counter_label),
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_key_reads_as_a_name_or_a_switch() {
        let named: CounterRef = serde_json::from_str(r#""acme.pending""#).unwrap();
        assert_eq!(named.source(), Some("acme.pending"));
        let off: CounterRef = serde_json::from_str("false").unwrap();
        assert_eq!(off, CounterRef::Off);
        let unset: CounterRef = serde_json::from_str("null").unwrap();
        assert!(unset.is_inherit());
        assert_eq!(serde_json::to_string(&off).unwrap(), "false");
        assert_eq!(serde_json::to_string(&named).unwrap(), r#""acme.pending""#);
    }

    #[test]
    fn a_large_count_is_capped() {
        assert_eq!(text(7), "7");
        assert_eq!(text(99), "99");
        assert_eq!(text(100), "99+");
    }
}
