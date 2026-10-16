//! Ordered hierarchies: one engine for every list things insert themselves
//! into (the menu, the settings index, a dashboard, a plugin's own list).
//!
//! An entry carries a [`Position`]: a `slot` (a named band that says what
//! kind of thing it is), an `order` inside the band, and an optional anchor
//! to a sibling. A [`Hierarchy`] owns a list: its policy says whether a
//! deployment and an operator may rearrange it, and which keys stay pinned at
//! the ends. [`arrange`] sorts, anchors and applies the overrides.

use serde::{Deserialize, Deserializer, Serialize};

/// The bands a slot is chosen from, on a hundreds scale: an author names the
/// one that matches what the thing is, and the lists read sensibly without
/// anyone reading the whole menu. Anything a plugin contributes without a
/// slot lands in [`slot::APPLICATION`].
pub mod slot {
    /// The home of the panel: the dashboard.
    pub const HOME: i32 = 0;
    /// What the site publishes: pages, posts, themes.
    pub const CONTENT: i32 = 100;
    /// Files: the media library.
    pub const MEDIA: i32 = 200;
    /// The application's own screens: the default for a plugin's resources.
    pub const APPLICATION: i32 = 500;
    /// Accounts and roles.
    pub const PEOPLE: i32 = 700;
    /// Reports, logs and tools an operator opens on occasion.
    pub const TOOLS: i32 = 800;
    /// The panel itself: settings.
    pub const SYSTEM: i32 = 900;
    /// After everything, pinned or not.
    pub const LAST: i32 = i32::MAX;

    /// The names a descriptor writes, as `slot: media`.
    pub const NAMES: [(&str, i32); 8] = [
        ("home", HOME),
        ("content", CONTENT),
        ("media", MEDIA),
        ("application", APPLICATION),
        ("people", PEOPLE),
        ("tools", TOOLS),
        ("system", SYSTEM),
        ("last", LAST),
    ];

    /// The band a name stands for.
    pub fn parse(name: &str) -> Option<i32> {
        NAMES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
            .map(|(_, v)| *v)
    }

    /// The name of a band, for the boot log and `lat doctor`.
    pub fn name(value: i32) -> String {
        NAMES
            .iter()
            .find(|(_, v)| *v == value)
            .map(|(n, _)| n.to_string())
            .unwrap_or_else(|| value.to_string())
    }
}

/// Where an entry sits in its hierarchy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Position {
    /// The band, by number; a descriptor writes a name (`slot: media`).
    #[serde(deserialize_with = "deserialize_slot")]
    pub slot: i32,
    /// Inside the band, lowest first; equal orders sort by label.
    pub order: i32,
    /// Sits right after the sibling with this key, whatever the bands say.
    pub after: Option<String>,
    /// Sits right before the sibling with this key.
    pub before: Option<String>,
}

impl Default for Position {
    fn default() -> Self {
        Self {
            slot: slot::APPLICATION,
            order: 0,
            after: None,
            before: None,
        }
    }
}

impl Position {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn slot(mut self, slot: i32) -> Self {
        self.slot = slot;
        self
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn after(mut self, key: impl Into<String>) -> Self {
        self.after = Some(key.into());
        self
    }

    pub fn before(mut self, key: impl Into<String>) -> Self {
        self.before = Some(key.into());
        self
    }
}

/// A slot as a descriptor writes it: a band's name, or a number.
pub fn deserialize_slot<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(i32),
        Name(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Number(n) => Ok(n),
        Raw::Name(name) => slot::parse(&name).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown slot {name:?}; one of {}",
                slot::NAMES
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }),
    }
}

/// Whether a hierarchy's arrangement may be changed after the author placed
/// its entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Policy {
    /// Rendered as declared; overrides are ignored and logged.
    Fixed,
    /// A deployment's `[backend.order]` and, when the hierarchy allows it, an
    /// operator's preference rearrange it.
    #[default]
    Adjustable,
}

/// A list entries are arranged in: its key names it in configuration, its
/// policy says who may rearrange it, and its pins hold the ends.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Hierarchy {
    /// `backend.nav`, `backend.settings`, or a plugin's own `vendor.name`.
    pub key: String,
    pub policy: Policy,
    /// Whether an operator's own preference may rearrange it, beyond the
    /// deployment's configuration.
    pub operator_adjustable: bool,
    /// Keys that stay first, in this order, whatever anyone says.
    pub pinned_first: Vec<String>,
    /// Keys that stay last, in this order.
    pub pinned_last: Vec<String>,
}

impl Hierarchy {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            policy: Policy::Adjustable,
            operator_adjustable: false,
            pinned_first: Vec::new(),
            pinned_last: Vec::new(),
        }
    }

    pub fn policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Lets an operator's preference rearrange it.
    pub fn operator_adjustable(mut self) -> Self {
        self.operator_adjustable = true;
        self
    }

    pub fn pin_first(mut self, key: impl Into<String>) -> Self {
        self.pinned_first.push(key.into());
        self
    }

    pub fn pin_last(mut self, key: impl Into<String>) -> Self {
        self.pinned_last.push(key.into());
        self
    }

    fn pinned(&self, key: &str) -> bool {
        self.pinned_first.iter().any(|k| k == key) || self.pinned_last.iter().any(|k| k == key)
    }
}

/// What an arranged entry exposes.
pub trait Arranged {
    /// Unique among its siblings: what an anchor or an override names.
    fn key(&self) -> &str;
    fn position(&self) -> &Position;
    /// The tie-break after slot and order.
    fn label(&self) -> &str;
}

/// The sequences that rearrange an adjustable hierarchy: the deployment's,
/// then the operator's on top. Each lists keys in the wanted order; keys left
/// out keep their arrangement after the listed ones, and a key that is no
/// entry is skipped.
#[derive(Clone, Debug, Default)]
pub struct Overrides<'a> {
    pub deployment: Option<&'a [String]>,
    pub operator: Option<&'a [String]>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ArrangeError {
    #[error("`{key}` sits {relation} `{anchor}`, which is no sibling")]
    UnknownAnchor {
        key: String,
        relation: &'static str,
        anchor: String,
    },
}

/// Arranges `items` for `hierarchy`: by slot, then order, then label; each
/// anchored entry moved next to its anchor; then the overrides the policy
/// admits; then the pins.
pub fn arrange<T: Arranged>(
    hierarchy: &Hierarchy,
    mut items: Vec<T>,
    overrides: Overrides<'_>,
) -> Result<Vec<T>, ArrangeError> {
    items.sort_by(|a, b| {
        let (pa, pb) = (a.position(), b.position());
        pa.slot
            .cmp(&pb.slot)
            .then_with(|| pa.order.cmp(&pb.order))
            .then_with(|| a.label().cmp(b.label()))
    });
    anchor(&mut items)?;
    if hierarchy.policy == Policy::Adjustable {
        if let Some(sequence) = overrides.deployment {
            reorder(hierarchy, &mut items, sequence);
        }
        if hierarchy.operator_adjustable {
            if let Some(sequence) = overrides.operator {
                reorder(hierarchy, &mut items, sequence);
            }
        }
    } else if overrides.deployment.is_some() || overrides.operator.is_some() {
        tracing::info!(hierarchy = %hierarchy.key, "arrangement is fixed; an override was ignored");
    }
    pin(hierarchy, &mut items);
    Ok(items)
}

/// Moves each anchored entry next to its anchor, in order of appearance, so
/// `after` holds whatever the slots say. An anchor that is no sibling is
/// refused: a typo would otherwise be silent.
fn anchor<T: Arranged>(items: &mut Vec<T>) -> Result<(), ArrangeError> {
    let keys: Vec<String> = items.iter().map(|i| i.key().to_string()).collect();
    for key in keys {
        let Some(at) = items.iter().position(|i| i.key() == key) else {
            continue;
        };
        let (anchor_key, after) = match (&items[at].position().after, &items[at].position().before)
        {
            (Some(a), _) => (a.clone(), true),
            (None, Some(b)) => (b.clone(), false),
            (None, None) => continue,
        };
        let Some(target) = items.iter().position(|i| i.key() == anchor_key) else {
            return Err(ArrangeError::UnknownAnchor {
                key,
                relation: if after { "after" } else { "before" },
                anchor: anchor_key,
            });
        };
        let moved = items.remove(at);
        let target = items
            .iter()
            .position(|i| i.key() == anchor_key)
            .unwrap_or(target);
        let to = if after { target + 1 } else { target };
        items.insert(to.min(items.len()), moved);
    }
    Ok(())
}

/// Puts the keys in `sequence` first, as listed, then the rest as they were.
/// Pinned keys are left for [`pin`]; a key that is no entry is skipped.
pub fn reorder<T: Arranged>(hierarchy: &Hierarchy, items: &mut Vec<T>, sequence: &[String]) {
    let mut rest: Vec<T> = std::mem::take(items);
    let mut ordered: Vec<T> = Vec::with_capacity(rest.len());
    for key in sequence {
        if hierarchy.pinned(key) {
            continue;
        }
        if let Some(at) = rest.iter().position(|i| i.key() == key) {
            ordered.push(rest.remove(at));
        }
    }
    ordered.extend(rest);
    *items = ordered;
}

/// Moves the pinned keys to the ends, in the hierarchy's order.
fn pin<T: Arranged>(hierarchy: &Hierarchy, items: &mut Vec<T>) {
    let mut rest: Vec<T> = std::mem::take(items);
    let mut first: Vec<T> = Vec::new();
    for key in &hierarchy.pinned_first {
        if let Some(at) = rest.iter().position(|i| i.key() == key) {
            first.push(rest.remove(at));
        }
    }
    let mut last: Vec<T> = Vec::new();
    for key in &hierarchy.pinned_last {
        if let Some(at) = rest.iter().position(|i| i.key() == key) {
            last.push(rest.remove(at));
        }
    }
    first.extend(rest);
    first.extend(last);
    *items = first;
}

/// A sequence as a preference or a config value stores it: comma-separated
/// keys, blanks dropped.
pub fn parse_sequence(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Entry {
        key: &'static str,
        label: &'static str,
        position: Position,
    }

    impl Arranged for Entry {
        fn key(&self) -> &str {
            self.key
        }
        fn position(&self) -> &Position {
            &self.position
        }
        fn label(&self) -> &str {
            self.label
        }
    }

    fn entry(key: &'static str, position: Position) -> Entry {
        Entry {
            key,
            label: key,
            position,
        }
    }

    fn keys(items: &[Entry]) -> Vec<&str> {
        items.iter().map(|e| e.key).collect()
    }

    fn menu() -> Hierarchy {
        Hierarchy::new("backend.nav")
            .pin_first("dashboard")
            .pin_last("settings")
            .operator_adjustable()
    }

    #[test]
    fn slots_order_the_bands_and_names_parse() {
        let items = vec![
            entry("acme.shop", Position::new()),
            entry("settings", Position::new().slot(slot::SYSTEM)),
            entry("media", Position::new().slot(slot::MEDIA)),
            entry("dashboard", Position::new().slot(slot::HOME)),
            entry("acme.blog", Position::new().order(-1)),
        ];
        let out = arrange(&menu(), items, Overrides::default()).unwrap();
        assert_eq!(
            keys(&out),
            ["dashboard", "media", "acme.blog", "acme.shop", "settings"]
        );
        assert_eq!(slot::parse("Media"), Some(slot::MEDIA));
        assert_eq!(slot::parse("nope"), None);
        assert_eq!(slot::name(slot::TOOLS), "tools");
        let p: Position = serde_json::from_str(r#"{"slot":"people","order":2}"#).unwrap();
        assert_eq!((p.slot, p.order), (slot::PEOPLE, 2));
        let p: Position = serde_json::from_str(r#"{"slot":150}"#).unwrap();
        assert_eq!(p.slot, 150);
        assert!(serde_json::from_str::<Position>(r#"{"slot":"attic"}"#).is_err());
    }

    #[test]
    fn an_anchor_holds_across_bands_and_a_missing_one_is_refused() {
        let items = vec![
            entry("media", Position::new().slot(slot::MEDIA)),
            entry("acme.shop", Position::new()),
            entry(
                "acme.reports",
                Position::new().slot(slot::TOOLS).before("media"),
            ),
        ];
        let out = arrange(&Hierarchy::new("h"), items, Overrides::default()).unwrap();
        assert_eq!(keys(&out), ["acme.reports", "media", "acme.shop"]);
        let items = vec![entry("a", Position::new().after("ghost"))];
        assert_eq!(
            arrange(&Hierarchy::new("h"), items, Overrides::default()).unwrap_err(),
            ArrangeError::UnknownAnchor {
                key: "a".into(),
                relation: "after",
                anchor: "ghost".into()
            }
        );
    }

    #[test]
    fn overrides_rearrange_an_adjustable_hierarchy_and_never_the_pins() {
        let items = || {
            vec![
                entry("dashboard", Position::new().slot(slot::HOME)),
                entry("media", Position::new().slot(slot::MEDIA)),
                entry("acme.shop", Position::new()),
                entry("acme.blog", Position::new()),
                entry("settings", Position::new().slot(slot::SYSTEM)),
            ]
        };
        let deployment = parse_sequence("acme.shop, media");
        let operator = parse_sequence("settings,acme.blog,gone,dashboard");
        let out = arrange(
            &menu(),
            items(),
            Overrides {
                deployment: Some(&deployment),
                operator: None,
            },
        )
        .unwrap();
        assert_eq!(
            keys(&out),
            ["dashboard", "acme.shop", "media", "acme.blog", "settings"]
        );
        let out = arrange(
            &menu(),
            items(),
            Overrides {
                deployment: Some(&deployment),
                operator: Some(&operator),
            },
        )
        .unwrap();
        assert_eq!(
            keys(&out),
            ["dashboard", "acme.blog", "acme.shop", "media", "settings"],
            "the operator's order wins, the ends stay"
        );
        let fixed = Hierarchy::new("h").policy(Policy::Fixed);
        let out = arrange(
            &fixed,
            items(),
            Overrides {
                deployment: Some(&deployment),
                operator: Some(&operator),
            },
        )
        .unwrap();
        assert_eq!(
            keys(&out),
            ["dashboard", "media", "acme.blog", "acme.shop", "settings"],
            "a fixed hierarchy ignores every override"
        );
        let out = arrange(
            &Hierarchy::new("h"),
            items(),
            Overrides {
                deployment: None,
                operator: Some(&operator),
            },
        )
        .unwrap();
        assert_eq!(
            keys(&out),
            ["dashboard", "media", "acme.blog", "acme.shop", "settings"],
            "an operator's order needs the hierarchy to allow it"
        );
    }
}
