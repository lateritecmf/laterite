//! The checklist: checkboxes in groups, one component for every screen.
//!
//! A screen describes the list; the component decides how to present it from
//! its size. Past [`SELECT_ALL_PAST`] choices it offers select all and none,
//! past [`SEARCH_PAST`] a search box, and past [`COLLAPSE_PAST`] choices in
//! more than one group it closes the groups that are settled (every choice
//! ticked, or none) and opens the ones partly ticked. Each decision has an
//! override: [`Checklist::select_all`], [`Checklist::search`],
//! [`Checklist::expand`].
//!
//! ```
//! use laterite_admin::checklist::{Checklist, Choice};
//! use laterite_core::Translator;
//!
//! let html = Checklist::new("Topics")
//!     .choice(Choice::new("topic", "news", "News").checked(true))
//!     .group("Sport", vec![
//!         Choice::new("topic", "football", "Football").into(),
//!         Choice::new("topic", "cricket", "Cricket").note("Tests and one-day").into(),
//!     ])
//!     .render(&Translator::new("en"));
//! assert!(html.as_str().contains(r#"name="topic" value="football""#));
//! ```
//!
//! The markup works before any script runs: the boxes submit and the groups
//! open and close. The `checklist` island adds the group boxes, the counts that
//! follow the ticks, the search and the `lat:checklist:changed` event.

use askama::Template;
use laterite_core::{t, Translator};
use serde::{Deserialize, Serialize};

use crate::html::Markup;

/// Select all and select none are offered past this many choices.
pub const SELECT_ALL_PAST: usize = 10;
/// The search box is offered past this many choices.
pub const SEARCH_PAST: usize = 20;
/// Settled groups start closed past this many choices.
pub const COLLAPSE_PAST: usize = 10;

/// Whether a part of the checklist is offered: decided from the size of the
/// list, or stated. In a descriptor file: `auto`, `true`, `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Offer {
    /// Offered when the list is long enough to need it.
    #[default]
    Auto,
    Always,
    Never,
}

impl Offer {
    fn decide(self, total: usize, past: usize) -> bool {
        match self {
            Offer::Auto => total > past,
            Offer::Always => true,
            Offer::Never => false,
        }
    }
}

impl Serialize for Offer {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Offer::Auto => s.serialize_str("auto"),
            Offer::Always => s.serialize_bool(true),
            Offer::Never => s.serialize_bool(false),
        }
    }
}

impl<'de> Deserialize<'de> for Offer {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct Seen;
        impl serde::de::Visitor<'_> for Seen {
            type Value = Offer;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("`auto`, `true` or `false`")
            }
            fn visit_bool<E: serde::de::Error>(self, on: bool) -> Result<Offer, E> {
                Ok(if on { Offer::Always } else { Offer::Never })
            }
            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Offer, E> {
                match text {
                    "auto" => Ok(Offer::Auto),
                    "true" => Ok(Offer::Always),
                    "false" => Ok(Offer::Never),
                    other => Err(E::invalid_value(serde::de::Unexpected::Str(other), &self)),
                }
            }
        }
        de.deserialize_any(Seen)
    }
}

/// Which groups start open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expand {
    /// A long list closes its settled groups and opens the partly ticked ones;
    /// a short list, or one with a single group, opens everything.
    #[default]
    Auto,
    /// Every group open.
    All,
    /// Every group closed.
    None,
}

/// One checkbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Choice {
    /// The name the box submits under.
    pub name: String,
    /// The value it submits.
    pub value: String,
    pub label: String,
    /// A second line under the label.
    pub note: Option<String>,
    /// An identifier shown beside the label, in the code face.
    pub code: Option<String>,
    pub checked: bool,
    /// Shown, and not changeable: the box is disabled, so the browser leaves it
    /// out of the submission whatever its state.
    pub locked: bool,
}

impl Choice {
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            label: label.into(),
            note: None,
            code: None,
            checked: false,
            locked: false,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }
}

/// A heading over entries of its own. Its box is ticked when all of them are,
/// clear when none is, and in between otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Group {
    pub label: String,
    pub entries: Vec<Entry>,
}

impl Group {
    pub fn new(label: impl Into<String>, entries: Vec<Entry>) -> Self {
        Self {
            label: label.into(),
            entries,
        }
    }
}

/// A line of the checklist: a choice, or a group of lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Entry {
    Choice(Choice),
    Group(Group),
}

impl From<Choice> for Entry {
    fn from(choice: Choice) -> Self {
        Entry::Choice(choice)
    }
}

impl From<Group> for Entry {
    fn from(group: Group) -> Self {
        Entry::Group(group)
    }
}

impl Entry {
    /// `(ticked, total)` over the choices at and under this entry.
    fn tally(&self) -> (usize, usize) {
        match self {
            Entry::Choice(choice) => (usize::from(choice.checked), 1),
            Entry::Group(group) => tally(&group.entries),
        }
    }
}

fn tally(entries: &[Entry]) -> (usize, usize) {
    entries
        .iter()
        .map(Entry::tally)
        .fold((0, 0), |sum, one| (sum.0 + one.0, sum.1 + one.1))
}

/// A checklist, as a screen describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checklist {
    label: String,
    id: Option<String>,
    entries: Vec<Entry>,
    select_all: Offer,
    search: Offer,
    expand: Expand,
    read_only: bool,
}

impl Checklist {
    /// An empty checklist. `label` names it to a screen reader.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            id: None,
            entries: Vec::new(),
            select_all: Offer::Auto,
            search: Offer::Auto,
            expand: Expand::Auto,
            read_only: false,
        }
    }

    /// The element id of the list, for a link or a label to point at.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Adds a choice at the top level.
    pub fn choice(mut self, choice: Choice) -> Self {
        self.entries.push(Entry::Choice(choice));
        self
    }

    /// Adds a group at the top level.
    pub fn group(mut self, label: impl Into<String>, entries: Vec<Entry>) -> Self {
        self.entries.push(Entry::Group(Group::new(label, entries)));
        self
    }

    /// Adds an entry built elsewhere.
    pub fn entry(mut self, entry: impl Into<Entry>) -> Self {
        self.entries.push(entry.into());
        self
    }

    /// Select all and select none. Default: past [`SELECT_ALL_PAST`] choices.
    pub fn select_all(mut self, offer: Offer) -> Self {
        self.select_all = offer;
        self
    }

    /// The search box. Default: past [`SEARCH_PAST`] choices.
    pub fn search(mut self, offer: Offer) -> Self {
        self.search = offer;
        self
    }

    /// Which groups start open. Default: [`Expand::Auto`].
    pub fn expand(mut self, expand: Expand) -> Self {
        self.expand = expand;
        self
    }

    /// Shows the list without letting it change: every box is locked and
    /// select all is withheld.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// How many choices the list holds, at every depth.
    pub fn total(&self) -> usize {
        tally(&self.entries).1
    }

    /// How many of them are ticked.
    pub fn checked(&self) -> usize {
        tally(&self.entries).0
    }

    /// Resolves the list for rendering: every decision made, every string in
    /// the reader's language. Serialisable, so a field type carries it in its
    /// view-model and an override presents the same list.
    pub fn view(&self, i18n: &Translator) -> ChecklistView {
        let (checked, total) = tally(&self.entries);
        let groups = self
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Group(_)))
            .count();
        let closes = total > COLLAPSE_PAST && groups > 1;
        // The pattern goes to the page with its placeholders intact, in the
        // word order of the reader's language, for the island to fill in.
        let pattern = i18n.t(&t!("{n} of {total}", n = "{n}", total = "{total}"));
        let nodes = self
            .entries
            .iter()
            .map(|e| node(e, self.expand, closes, self.read_only, &pattern))
            .collect();
        ChecklistView {
            label: self.label.clone(),
            id: self.id.clone(),
            nodes,
            select_all: !self.read_only && self.select_all.decide(total, SELECT_ALL_PAST),
            search: self.search.decide(total, SEARCH_PAST),
            read_only: self.read_only,
            count: count(&pattern, checked, total),
            pattern,
            text_select_all: i18n.t(&t!("Select all")),
            text_select_none: i18n.t(&t!("Select none")),
            text_search: i18n.t(&t!("Search")),
            text_no_match: i18n.t(&t!("Nothing matches.")),
        }
    }

    /// The checklist as markup.
    pub fn render(&self, i18n: &Translator) -> Markup {
        self.view(i18n).render_markup()
    }
}

fn count(pattern: &str, n: usize, total: usize) -> String {
    pattern
        .replace("{n}", &n.to_string())
        .replace("{total}", &total.to_string())
}

fn node(entry: &Entry, expand: Expand, closes: bool, read_only: bool, pattern: &str) -> NodeView {
    match entry {
        Entry::Choice(choice) => NodeView {
            group: false,
            label: choice.label.clone(),
            name: choice.name.clone(),
            value: choice.value.clone(),
            note: choice.note.clone(),
            code: choice.code.clone(),
            checked: choice.checked,
            locked: read_only || choice.locked,
            open: false,
            count: String::new(),
            nodes: Vec::new(),
        },
        Entry::Group(group) => {
            let (checked, total) = tally(&group.entries);
            let partly = checked > 0 && checked < total;
            NodeView {
                group: true,
                label: group.label.clone(),
                name: String::new(),
                value: String::new(),
                note: None,
                code: None,
                checked: total > 0 && checked == total,
                locked: read_only,
                open: match expand {
                    Expand::All => true,
                    Expand::None => false,
                    Expand::Auto => !closes || partly,
                },
                count: count(pattern, checked, total),
                nodes: group
                    .entries
                    .iter()
                    .map(|e| node(e, expand, closes, read_only, pattern))
                    .collect(),
            }
        }
    }
}

/// A checklist resolved for rendering. Built by [`Checklist::view`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, Template)]
#[template(path = "_checklist.html")]
pub struct ChecklistView {
    label: String,
    id: Option<String>,
    nodes: Vec<NodeView>,
    select_all: bool,
    search: bool,
    read_only: bool,
    /// The count over the whole list, as first rendered.
    count: String,
    /// The count's pattern, `{n}` and `{total}` intact.
    pattern: String,
    text_select_all: String,
    text_select_none: String,
    text_search: String,
    text_no_match: String,
}

impl ChecklistView {
    /// The view as markup.
    pub fn render_markup(&self) -> Markup {
        Markup::from_template(self).unwrap_or_default()
    }

    /// Whether the bar above the list has anything in it.
    fn bar(&self) -> bool {
        self.select_all || self.search
    }
}

/// One line of a resolved checklist: a choice, or a group with lines of its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Template)]
#[template(path = "_checklist_node.html")]
struct NodeView {
    group: bool,
    label: String,
    name: String,
    value: String,
    note: Option<String>,
    code: Option<String>,
    checked: bool,
    locked: bool,
    open: bool,
    count: String,
    nodes: Vec<NodeView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> Translator {
        Translator::new("en")
    }

    fn choices(prefix: &str, n: usize, ticked: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| {
                Choice::new("topic", format!("{prefix}{i}"), format!("{prefix} {i}"))
                    .checked(i < ticked)
                    .into()
            })
            .collect()
    }

    fn opens(view: &ChecklistView) -> Vec<bool> {
        view.nodes
            .iter()
            .filter(|n| n.group)
            .map(|n| n.open)
            .collect()
    }

    #[test]
    fn a_short_list_offers_no_bar_and_opens_every_group() {
        let list = Checklist::new("Topics")
            .group("One", choices("a", 3, 0))
            .group("Two", choices("b", 3, 3));
        let view = list.view(&en());
        assert!(!view.select_all && !view.search);
        assert_eq!(opens(&view), [true, true]);
        assert_eq!(view.count, "3 of 6");
    }

    #[test]
    fn a_long_list_closes_what_is_settled_and_opens_what_is_in_progress() {
        let list = Checklist::new("Topics")
            .group("None ticked", choices("a", 5, 0))
            .group("Some ticked", choices("b", 5, 2))
            .group("All ticked", choices("c", 5, 5));
        let view = list.view(&en());
        assert_eq!(opens(&view), [false, true, false]);
        assert!(view.select_all, "past ten choices");
        assert!(!view.search, "not past twenty");
        let counts: Vec<&str> = view.nodes.iter().map(|n| n.count.as_str()).collect();
        assert_eq!(counts, ["0 of 5", "2 of 5", "5 of 5"]);
        assert!(view.nodes[2].checked && !view.nodes[1].checked);
    }

    #[test]
    fn a_long_list_in_one_group_stays_open() {
        let view = Checklist::new("Topics")
            .group("Only", choices("a", 25, 0))
            .view(&en());
        assert_eq!(opens(&view), [true]);
        assert!(view.search, "past twenty choices");
    }

    #[test]
    fn every_decision_has_an_override() {
        let list = Checklist::new("Topics")
            .group("One", choices("a", 2, 0))
            .group("Two", choices("b", 2, 1))
            .select_all(Offer::Always)
            .search(Offer::Always)
            .expand(Expand::None);
        let view = list.view(&en());
        assert!(view.select_all && view.search);
        assert_eq!(opens(&view), [false, false]);

        let list = Checklist::new("Topics")
            .group("One", choices("a", 15, 0))
            .group("Two", choices("b", 15, 15))
            .select_all(Offer::Never)
            .search(Offer::Never)
            .expand(Expand::All);
        let view = list.view(&en());
        assert!(!view.select_all && !view.search);
        assert_eq!(opens(&view), [true, true]);
    }

    #[test]
    fn a_group_inside_a_group_counts_toward_both() {
        let inner = Group::new("Inner", choices("b", 2, 1));
        let mut outer = choices("a", 2, 2);
        outer.push(inner.into());
        let list = Checklist::new("Topics").group("Outer", outer);
        assert_eq!((list.checked(), list.total()), (3, 4));
        let view = list.view(&en());
        assert_eq!(view.nodes[0].count, "3 of 4");
        assert_eq!(view.nodes[0].nodes[2].count, "1 of 2");

        let html = list.render(&en());
        let html = html.as_str();
        assert_eq!(html.matches("<details").count(), 2);
        assert_eq!(html.matches(r#"name="topic""#).count(), 4);
    }

    #[test]
    fn a_read_only_list_locks_every_box_and_withholds_select_all() {
        let list = Checklist::new("Topics")
            .group("One", choices("a", 12, 12))
            .read_only(true);
        let view = list.view(&en());
        assert!(!view.select_all);
        let html = list.render(&en());
        let html = html.as_str();
        assert_eq!(
            html.matches(" disabled").count(),
            13,
            "twelve boxes and the group's"
        );
        assert!(!html.contains("data-lat-checklist-all"));
    }

    #[test]
    fn the_markup_escapes_what_a_screen_hands_it() {
        let html = Checklist::new("Topics")
            .choice(
                Choice::new("topic", "a\"b", "<b>Bold</b>")
                    .note("1 < 2")
                    .code("x&y"),
            )
            .render(&en());
        let html = html.as_str();
        assert!(!html.contains("<b>Bold</b>"));
        assert!(html.contains("&lt;b&gt;Bold&lt;/b&gt;") || html.contains("&#60;b&#62;"));
        assert!(!html.contains(r#"value="a"b""#));
    }

    #[test]
    fn offer_reads_a_word_or_a_boolean() {
        let read = |text: &str| serde_json::from_str::<Offer>(text);
        assert_eq!(read("\"auto\"").unwrap(), Offer::Auto);
        assert_eq!(read("true").unwrap(), Offer::Always);
        assert_eq!(read("false").unwrap(), Offer::Never);
        assert!(read("\"sometimes\"").is_err());
        assert_eq!(serde_json::to_string(&Offer::Always).unwrap(), "true");
    }
}
