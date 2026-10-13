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
    /// Sorts among siblings, lowest first; equal orders sort by label.
    pub order: i32,
    /// The key of the item this sits under; `None` is the top bar.
    pub under: Option<String>,
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
            order: 0,
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
            order: 0,
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

    /// Places the entry under the section with this key.
    pub fn under(mut self, section: impl Into<String>) -> Self {
        self.under = Some(section.into());
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct Placement {
    /// The key of the section to sit under; `None` is the top bar.
    pub under: Option<String>,
    pub icon: Option<String>,
    pub order: i32,
    /// Mounted and reachable by link, with no menu entry.
    pub hidden: bool,
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
}

/// `menu: false` hides; `menu: { under, icon, order }` places.
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
}

/// The menu as a tree, built once at boot.
#[derive(Debug, Default)]
pub(crate) struct NavTree {
    pub top: Vec<NavNode>,
}

impl NavTree {
    /// Resolves `items` into a tree under `admin_path`. Refuses a duplicate
    /// key, an entry under a key nobody declared, and an entry under an entry
    /// (one level of sections).
    pub(crate) fn build(items: Vec<NavItem>, admin_path: &str) -> Result<Self, String> {
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
            .map(|i| NavNode {
                item: prefixed(i),
                children: Vec::new(),
            })
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
            parent.children.push(NavNode {
                item: prefixed(item),
                children: Vec::new(),
            });
        }
        let by_order = |a: &NavNode, b: &NavNode| {
            a.item
                .order
                .cmp(&b.item.order)
                .then_with(|| a.item.label.source().cmp(b.item.label.source()))
        };
        top.sort_by(by_order);
        for node in &mut top {
            node.children.sort_by(by_order);
        }
        Ok(Self { top })
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
                })
            })
            .collect()
    }
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
                label: tr.t(&c.item.label),
                description: String::new(),
                path: c.item.path.clone().unwrap_or_default(),
                icon: icons.render(c.item.icon.as_deref()),
                active: active.as_deref() == Some(c.item.key.as_str()),
            })
            .collect(),
    )]
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
