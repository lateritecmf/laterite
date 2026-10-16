//! The admin's menu: a registry of items that modules, resources and screens
//! contribute as data, resolved once at boot into a tree and filtered per
//! request by what the operator may see.
//!
//! A top-level item is a tab in the top bar. An item placed under another
//! makes that one a section: the tab leads to the section's first visible
//! entry and the entries show in the context sidebar. Every item carries the
//! permission its target requires, so an operator never sees a tab that would
//! refuse them.

use laterite_core::Text;
use serde::{Deserialize, Serialize};

use crate::settings::{CategoryView, ItemView};

/// One menu entry: a tab, or an entry under a section.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct NavItem {
    /// Unique among every item (`acme.posts`): what another item names in
    /// `under`.
    pub key: String,
    pub label: Text,
    /// The path under the admin mount (`/posts`). A section may have none: its
    /// tab then leads to its first visible entry.
    pub path: Option<String>,
    /// An icon name from the icon set.
    pub icon: Option<String>,
    /// What the operator must hold to see the entry. `None` shows it to every
    /// signed-in operator; a section with no path shows when any entry does.
    pub permission: Option<String>,
    /// The band the entry belongs to (`slot: media`); see
    /// [`laterite_core::arrange::slot`]. A contributed entry defaults to the
    /// application band.
    #[serde(deserialize_with = "laterite_core::arrange::deserialize_slot")]
    pub slot: i32,
    /// Sorts among siblings of the band, lowest first; equal orders sort by label.
    pub order: i32,
    /// Sits right after the sibling with this key, whatever the bands say.
    pub after: Option<String>,
    /// Sits right before the sibling with this key, whatever the bands say.
    pub before: Option<String>,
    /// The key of the item this sits under; `None` is the top bar.
    pub under: Option<String>,
}

impl laterite_core::arrange::Arranged for NavNode {
    fn key(&self) -> &str {
        &self.item.key
    }
    fn position(&self) -> &laterite_core::arrange::Position {
        &self.position
    }
    fn label(&self) -> &str {
        self.item.label.source()
    }
}

impl NavItem {
    /// An entry leading to `path`.
    pub fn new(key: impl Into<String>, label: impl Into<Text>, path: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            path: Some(path.into()),
            icon: None,
            permission: None,
            slot: laterite_core::arrange::slot::APPLICATION,
            order: 0,
            after: None,
            before: None,
            under: None,
        }
    }

    /// A section: a tab that holds entries and leads to the first one.
    pub fn section(key: impl Into<String>, label: impl Into<Text>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            path: None,
            icon: None,
            permission: None,
            slot: laterite_core::arrange::slot::APPLICATION,
            order: 0,
            after: None,
            before: None,
            under: None,
        }
    }

    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// The band the entry belongs to; see [`laterite_core::arrange::slot`].
    pub fn slot(mut self, slot: i32) -> Self {
        self.slot = slot;
        self
    }

    /// Places the entry under the section with this key.
    pub fn under(mut self, section: impl Into<String>) -> Self {
        self.under = Some(section.into());
        self
    }

    /// Sits right after the sibling with this key.
    pub fn after(mut self, key: impl Into<String>) -> Self {
        self.after = Some(key.into());
        self
    }

    /// Sits right before the sibling with this key.
    pub fn before(mut self, key: impl Into<String>) -> Self {
        self.before = Some(key.into());
        self
    }
}

impl Default for NavItem {
    fn default() -> Self {
        Self::section("", "")
    }
}

/// Where a resource or a screen sits in the menu: under which section, with
/// which icon, at which position; or out of the menu altogether.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct Placement {
    /// The key of the section to sit under; `None` is the top bar.
    pub under: Option<String>,
    pub icon: Option<String>,
    /// The band (`slot: media`); the application band when unset.
    #[serde(deserialize_with = "laterite_core::arrange::deserialize_slot")]
    pub slot: i32,
    pub order: i32,
    /// Sits right after the sibling with this key (a resource's key is
    /// `resource:` and its path, a screen's `screen:` and its path).
    pub after: Option<String>,
    /// Sits right before the sibling with this key.
    pub before: Option<String>,
    /// Mounted and reachable by link, with no menu entry.
    pub hidden: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            under: None,
            icon: None,
            slot: laterite_core::arrange::slot::APPLICATION,
            order: 0,
            after: None,
            before: None,
            hidden: false,
        }
    }
}

impl Placement {
    pub fn new() -> Self {
        Self::default()
    }

    /// Out of the menu: the routes mount, a link elsewhere reaches them.
    pub fn hidden() -> Self {
        Self {
            hidden: true,
            ..Self::default()
        }
    }

    pub fn under(mut self, section: impl Into<String>) -> Self {
        self.under = Some(section.into());
        self
    }

    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// The band the entry belongs to; see [`laterite_core::arrange::slot`].
    pub fn slot(mut self, slot: i32) -> Self {
        self.slot = slot;
        self
    }

    /// Sits right after the sibling with this key.
    pub fn after(mut self, key: impl Into<String>) -> Self {
        self.after = Some(key.into());
        self
    }

    /// Sits right before the sibling with this key.
    pub fn before(mut self, key: impl Into<String>) -> Self {
        self.before = Some(key.into());
        self
    }
}

/// `menu: false` hides; `menu: { under, icon, order, after, before }` places.
pub(crate) fn deserialize_placement<'de, D>(deserializer: D) -> Result<Placement, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Key {
        Flag(bool),
        Config(Placement),
    }
    Ok(match Option::<Key>::deserialize(deserializer)? {
        Some(Key::Flag(false)) => Placement::hidden(),
        Some(Key::Flag(true)) | None => Placement::default(),
        Some(Key::Config(placement)) => placement,
    })
}

/// One resolved item: its entries beneath it, paths under the admin mount.
#[derive(Clone, Debug)]
pub(crate) struct NavNode {
    pub item: NavItem,
    pub children: Vec<NavNode>,
    /// The item's placement as the arrangement engine reads it.
    pub position: laterite_core::arrange::Position,
}

impl NavNode {
    fn leaf(item: NavItem) -> Self {
        let mut position = laterite_core::arrange::Position::new()
            .slot(item.slot)
            .order(item.order);
        if let Some(after) = &item.after {
            position = position.after(after.clone());
        }
        if let Some(before) = &item.before {
            position = position.before(before.clone());
        }
        Self {
            item,
            children: Vec::new(),
            position,
        }
    }
}

/// The top bar as a hierarchy: a deployment and an operator may rearrange it,
/// Dashboard stays first and Settings last.
pub(crate) fn top_hierarchy() -> laterite_core::arrange::Hierarchy {
    laterite_core::arrange::Hierarchy::new(TOP_HIERARCHY)
        .operator_adjustable()
        .pin_first("dashboard")
        .pin_last("settings")
}

/// The configuration key of the top bar.
pub const TOP_HIERARCHY: &str = "backend.nav";

/// The menu as a tree, built once at boot.
#[derive(Debug, Default)]
pub(crate) struct NavTree {
    pub top: Vec<NavNode>,
}

impl NavTree {
    /// Resolves `items` into a tree under `admin_path`. Refuses a duplicate
    /// key, an entry under a key nobody declared, and an entry under an entry
    /// (one level of sections).
    #[cfg(test)]
    pub(crate) fn build(items: Vec<NavItem>, admin_path: &str) -> Result<Self, String> {
        Self::build_with(items, admin_path, &std::collections::BTreeMap::new())
    }

    /// [`build`](Self::build) with a deployment's `[backend.order]`: the top
    /// bar under `backend.nav`, a section's entries under `backend.nav.<key>`.
    pub(crate) fn build_with(
        items: Vec<NavItem>,
        admin_path: &str,
        order: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> Result<Self, String> {
        use laterite_core::arrange::{arrange, Hierarchy, Overrides};
        let mut seen = std::collections::HashSet::new();
        for item in &items {
            if item.key.is_empty() {
                return Err(format!("menu item `{}` has no key", item.label.source()));
            }
            if !seen.insert(item.key.clone()) {
                return Err(format!("duplicate menu key `{}`", item.key));
            }
        }
        let prefixed = |item: &NavItem| {
            let mut item = item.clone();
            item.path = item.path.map(|p| format!("{admin_path}{p}"));
            item
        };
        let mut top: Vec<NavNode> = items
            .iter()
            .filter(|i| i.under.is_none())
            .map(|i| NavNode::leaf(prefixed(i)))
            .collect();
        for item in items.iter().filter(|i| i.under.is_some()) {
            let under = item.under.as_deref().unwrap_or_default();
            let parent = top
                .iter_mut()
                .find(|n| n.item.key == under)
                .ok_or_else(|| {
                    format!(
                        "menu item `{}` sits under `{under}`, which is no top-level item",
                        item.key
                    )
                })?;
            parent.children.push(NavNode::leaf(prefixed(item)));
        }
        let overrides = |key: &str| Overrides {
            deployment: order.get(key).map(Vec::as_slice),
            operator: None,
        };
        let top = arrange(&top_hierarchy(), top, overrides(TOP_HIERARCHY))
            .map_err(|e| format!("menu: {e}"))?;
        let mut arranged = Vec::with_capacity(top.len());
        for mut node in top {
            let key = format!("{TOP_HIERARCHY}.{}", node.item.key);
            let children = std::mem::take(&mut node.children);
            node.children = arrange(&Hierarchy::new(&key), children, overrides(&key))
                .map_err(|e| format!("menu, under `{}`: {e}", node.item.key))?;
            arranged.push(node);
        }
        Ok(Self { top: arranged })
    }

    /// The tree as one operator sees it: entries they may not open are gone,
    /// and a section with nothing left beneath it and no page of its own goes
    /// too.
    pub(crate) fn visible(&self, allows: &dyn Fn(&str) -> bool) -> Vec<NavNode> {
        let allowed = |item: &NavItem| item.permission.as_deref().is_none_or(allows);
        self.top
            .iter()
            .filter_map(|node| {
                let children: Vec<NavNode> = node
                    .children
                    .iter()
                    .filter(|c| allowed(&c.item))
                    .cloned()
                    .collect();
                let own = node.item.path.is_some() && allowed(&node.item);
                if !own && children.is_empty() {
                    return None;
                }
                if node.item.path.is_some() && !allowed(&node.item) {
                    return None;
                }
                Some(NavNode {
                    item: node.item.clone(),
                    children,
                    position: node.position.clone(),
                })
            })
            .collect()
    }
}

/// The preference key holding an operator's own order for the top bar.
pub(crate) const ORDER_PREFERENCE: &str = "menu.order";

/// The preference key holding an operator's own moves between the top bar
/// and the settings index: a JSON object of entry key to `nav` or `settings`.
pub(crate) const MOVES_PREFERENCE: &str = "menu.moves";

/// The preference key holding an operator's own order for a hierarchy: the
/// top bar keeps its first key, every other hierarchy is named.
pub(crate) fn order_preference(hierarchy: &str) -> String {
    if hierarchy == TOP_HIERARCHY {
        ORDER_PREFERENCE.to_string()
    } else {
        format!("{ORDER_PREFERENCE}.{hierarchy}")
    }
}

/// The hierarchies an operator may rearrange and move entries between.
pub(crate) const OPERATOR_HIERARCHIES: [&str; 2] = [TOP_HIERARCHY, crate::settings::HIERARCHY];

/// An operator's moves as stored (`{"backend.access_log": "nav"}`), as the
/// engine's moves: `nav` is the top bar, `settings` the settings index.
pub(crate) fn operator_moves(
    stored: &str,
) -> std::collections::BTreeMap<String, laterite_core::arrange::Move> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(stored)
    else {
        return Default::default();
    };
    map.into_iter()
        .filter_map(|(key, place)| {
            let hierarchy = match place.as_str()? {
                "nav" => TOP_HIERARCHY,
                "settings" => crate::settings::HIERARCHY,
                _ => return None,
            };
            let mut m = laterite_core::arrange::Move::default();
            m.hierarchy = hierarchy.to_string();
            Some((key, m))
        })
        .collect()
}

/// Puts the top-level nodes in the operator's own order: the keys in `order`
/// (comma-separated) first, as listed, then the rest as they were. Dashboard
/// stays first and Settings last whatever the order says, and a key that is
/// no tab is ignored, so a stale preference never hides anything.
pub(crate) fn apply_order(nodes: &mut Vec<NavNode>, order: &str) {
    let wanted = laterite_core::arrange::parse_sequence(order);
    if wanted.is_empty() {
        return;
    }
    let taken = std::mem::take(nodes);
    *nodes = laterite_core::arrange::arrange(
        &top_hierarchy(),
        taken,
        laterite_core::arrange::Overrides {
            deployment: None,
            operator: Some(&wanted),
        },
    )
    .unwrap_or_default();
}

/// The path a top-level node's tab leads to: its own, or its first entry's.
pub(crate) fn tab_path(node: &NavNode) -> Option<&str> {
    node.item
        .path
        .as_deref()
        .or_else(|| node.children.first().and_then(|c| c.item.path.as_deref()))
}

/// Whether `path` is on `target` or beneath it.
fn within(path: &str, target: &str, admin_path: &str) -> bool {
    path == target || (target != admin_path && path.starts_with(&format!("{target}/")))
}

/// The key of the top-level node `path` falls under, by the longest match
/// over its own path and its entries' paths.
pub(crate) fn active_top<'a>(
    visible: &'a [NavNode],
    admin_path: &str,
    path: &str,
) -> Option<&'a NavNode> {
    let mut best: Option<(&NavNode, usize)> = None;
    for node in visible {
        let candidates = node
            .item
            .path
            .iter()
            .chain(node.children.iter().filter_map(|c| c.item.path.as_ref()));
        for target in candidates {
            if within(path, target, admin_path) && best.is_none_or(|(_, len)| target.len() > len) {
                best = Some((node, target.len()));
            }
        }
    }
    best.map(|(node, _)| node)
}

/// The context sidebar for a section: its entries as one group, the one
/// `path` falls under marked active.
pub(crate) fn section_sidebar(
    node: &NavNode,
    admin_path: &str,
    path: &str,
    tr: &laterite_core::Translator,
    icons: crate::icons::Icons<'_>,
) -> Vec<CategoryView> {
    if node.children.is_empty() {
        return Vec::new();
    }
    let active = node
        .children
        .iter()
        .filter(|c| {
            c.item
                .path
                .as_deref()
                .is_some_and(|p| within(path, p, admin_path))
        })
        .max_by_key(|c| c.item.path.as_deref().map(str::len).unwrap_or(0))
        .map(|c| c.item.key.clone());
    vec![CategoryView::new(
        tr.t(&node.item.label),
        node.children
            .iter()
            .map(|c| ItemView {
                key: c.item.key.clone(),
                label: tr.t(&c.item.label),
                description: String::new(),
                path: c.item.path.clone().unwrap_or_default(),
                icon: icons.render(c.item.icon.as_deref()),
                active: active.as_deref() == Some(c.item.key.as_str()),
            })
            .collect(),
    )]
}

/// Applies a deployment's `[backend.menu]` moves. A menu entry named in a
/// move to `backend.settings` becomes a linked settings item; a settings item
/// moved to `backend.nav` (or `backend.nav.<section>`) becomes a menu entry;
/// a move within a hierarchy changes the band, the order or the section. An
/// entry keeps its label, icon, permission and target wherever it renders. A
/// key nobody declared, or a section (which has no page), is logged and left.
pub(crate) fn apply_moves(
    items: &mut Vec<NavItem>,
    settings: &mut Vec<crate::settings::SettingsItem>,
    moves: &std::collections::BTreeMap<String, laterite_core::arrange::Move>,
) {
    use laterite_core::arrange::slot;
    for (key, spec) in moves {
        let to_settings = spec.hierarchy == crate::settings::HIERARCHY;
        let to_nav = spec.hierarchy == TOP_HIERARCHY
            || spec.hierarchy.starts_with(&format!("{TOP_HIERARCHY}."));
        if !to_settings && !to_nav {
            tracing::warn!(key = %key, hierarchy = %spec.hierarchy, "a menu move names a hierarchy nothing renders");
            continue;
        }
        let section = spec
            .hierarchy
            .strip_prefix(&format!("{TOP_HIERARCHY}."))
            .map(str::to_string);
        if let Some(at) = items.iter().position(|i| &i.key == key) {
            if to_nav {
                let item = &mut items[at];
                item.under = section;
                if let Some(slot) = spec.slot {
                    item.slot = slot;
                }
                if let Some(order) = spec.order {
                    item.order = order;
                }
                continue;
            }
            let Some(path) = items[at].path.clone() else {
                tracing::warn!(key = %key, "a section has no page of its own and cannot move into settings");
                continue;
            };
            let item = items.remove(at);
            let band = spec.slot.unwrap_or(item.slot);
            let mut moved = crate::settings::SettingsItem::new(
                item.key.clone(),
                item.label.clone(),
                Vec::new(),
            )
            .category(laterite_core::Text::dynamic(
                spec.category
                    .clone()
                    .unwrap_or_else(|| title(&slot::name(band))),
            ))
            .slot(band)
            .order(spec.order.unwrap_or(item.order))
            .link(path);
            if let Some(icon) = item.icon.clone() {
                moved = moved.icon(icon);
            }
            if let Some(permission) = item.permission.clone() {
                moved = moved.permission(permission);
            }
            settings.push(moved);
            continue;
        }
        if let Some(at) = settings.iter().position(|i| &i.code == key) {
            if to_settings {
                let item = &mut settings[at];
                if let Some(slot) = spec.slot {
                    item.slot = slot;
                }
                if let Some(order) = spec.order {
                    item.order = order;
                }
                if let Some(category) = &spec.category {
                    item.category = laterite_core::Text::dynamic(category.clone());
                }
                continue;
            }
            let item = settings.remove(at);
            let path = item.link.clone().unwrap_or_else(|| item.route());
            let mut moved = NavItem::new(item.code.clone(), item.label.clone(), path)
                .slot(spec.slot.unwrap_or(item.slot))
                .order(spec.order.unwrap_or(item.order));
            moved.icon = item.icon.clone();
            moved.permission = item.permission.clone();
            moved.under = section;
            items.push(moved);
            continue;
        }
        tracing::warn!(key = %key, "a menu move names an entry nothing declared");
    }
}

/// `tools` as a heading: `Tools`.
fn title(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

impl NavTree {
    /// One boot log line per hierarchy, so an author sees where an entry
    /// landed without opening the panel.
    pub(crate) fn log_arrangement(&self) {
        let top: Vec<String> = self
            .top
            .iter()
            .map(|n| {
                format!(
                    "{} ({})",
                    n.item.key,
                    laterite_core::arrange::slot::name(n.item.slot)
                )
            })
            .collect();
        tracing::info!(hierarchy = TOP_HIERARCHY, arrangement = %top.join(" > "));
        for node in &self.top {
            if node.children.is_empty() {
                continue;
            }
            let keys: Vec<&str> = node.children.iter().map(|c| c.item.key.as_str()).collect();
            tracing::info!(hierarchy = %format!("{TOP_HIERARCHY}.{}", node.item.key), arrangement = %keys.join(" > "));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<NavItem> {
        vec![
            NavItem::new("dashboard", "Dashboard", "").order(i32::MIN),
            NavItem::section("content", "Content").order(10),
            NavItem::new("acme.posts", "Posts", "/posts")
                .under("content")
                .permission("acme.posts")
                .order(2),
            NavItem::new("acme.pages", "Pages", "/pages")
                .under("content")
                .permission("acme.pages")
                .order(1),
            NavItem::new("acme.shop", "Shop", "/shop")
                .permission("acme.shop")
                .order(20),
            NavItem::new("settings", "Settings", "/settings").order(i32::MAX),
        ]
    }

    #[test]
    fn a_tree_sorts_and_prefixes() {
        let tree = NavTree::build(items(), "/admin").unwrap();
        let keys: Vec<&str> = tree.top.iter().map(|n| n.item.key.as_str()).collect();
        assert_eq!(keys, ["dashboard", "content", "acme.shop", "settings"]);
        let content = &tree.top[1];
        let entries: Vec<&str> = content
            .children
            .iter()
            .map(|n| n.item.key.as_str())
            .collect();
        assert_eq!(
            entries,
            ["acme.pages", "acme.posts"],
            "by order, then label"
        );
        assert_eq!(
            content.children[0].item.path.as_deref(),
            Some("/admin/pages")
        );
        assert_eq!(tab_path(content), Some("/admin/pages"));
    }

    #[test]
    fn an_anchor_holds_whatever_the_orders_say() {
        let mut items = items();
        items.push(
            NavItem::new("acme.help", "Help", "/help")
                .order(0)
                .after("acme.shop"),
        );
        items.push(
            NavItem::new("acme.tags", "Tags", "/tags")
                .under("content")
                .order(99)
                .before("acme.pages"),
        );
        let tree = NavTree::build(items, "/admin").unwrap();
        let keys: Vec<&str> = tree.top.iter().map(|n| n.item.key.as_str()).collect();
        assert_eq!(
            keys,
            ["dashboard", "content", "acme.shop", "acme.help", "settings"]
        );
        let entries: Vec<&str> = tree.top[1]
            .children
            .iter()
            .map(|n| n.item.key.as_str())
            .collect();
        assert_eq!(entries, ["acme.tags", "acme.pages", "acme.posts"]);

        let mut lost = self::items();
        lost.push(NavItem::new("acme.help", "Help", "/help").after("acme.pages"));
        assert!(
            NavTree::build(lost, "/admin").is_err(),
            "an anchor must be a sibling"
        );
    }

    #[test]
    fn an_operators_order_moves_tabs_but_never_the_ends() {
        let tree = NavTree::build(items(), "/admin").unwrap();
        let mut visible = tree.visible(&|_| true);
        apply_order(&mut visible, "settings,acme.shop,gone,dashboard");
        let keys: Vec<&str> = visible.iter().map(|n| n.item.key.as_str()).collect();
        assert_eq!(keys, ["dashboard", "acme.shop", "content", "settings"]);
        apply_order(&mut visible, "");
        let keys: Vec<&str> = visible.iter().map(|n| n.item.key.as_str()).collect();
        assert_eq!(
            keys,
            ["dashboard", "acme.shop", "content", "settings"],
            "blank changes nothing"
        );
    }

    #[test]
    fn a_duplicate_key_and_an_unknown_section_are_refused() {
        let mut dup = items();
        dup.push(NavItem::new("acme.shop", "Again", "/again"));
        assert!(NavTree::build(dup, "/admin").is_err());
        let mut orphan = items();
        orphan.push(NavItem::new("acme.lost", "Lost", "/lost").under("nowhere"));
        assert!(NavTree::build(orphan, "/admin").is_err());
    }

    #[test]
    fn an_operator_sees_only_what_they_may_open() {
        let tree = NavTree::build(items(), "/admin").unwrap();
        let visible = tree.visible(&|code| code == "acme.posts");
        let keys: Vec<&str> = visible.iter().map(|n| n.item.key.as_str()).collect();
        assert_eq!(keys, ["dashboard", "content", "settings"], "no shop");
        assert_eq!(visible[1].children.len(), 1, "only posts under content");
        assert_eq!(tab_path(&visible[1]), Some("/admin/posts"));
        let none = tree.visible(&|_| false);
        let keys: Vec<&str> = none.iter().map(|n| n.item.key.as_str()).collect();
        assert_eq!(keys, ["dashboard", "settings"], "an empty section goes");
    }

    #[test]
    fn the_active_tab_is_the_longest_match() {
        let tree = NavTree::build(items(), "/admin").unwrap();
        let visible = tree.visible(&|_| true);
        let at = |path: &str| active_top(&visible, "/admin", path).map(|n| n.item.key.clone());
        assert_eq!(at("/admin/posts/3/edit").as_deref(), Some("content"));
        assert_eq!(at("/admin/shop").as_deref(), Some("acme.shop"));
        assert_eq!(at("/admin/settings/users").as_deref(), Some("settings"));
        assert_eq!(at("/admin").as_deref(), Some("dashboard"));
        assert_eq!(at("/admin/elsewhere"), None);
    }
}
