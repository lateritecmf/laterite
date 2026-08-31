//! The extension registry.
//!
//! Modules contribute typed items in their [`register`](crate::module::Module::register);
//! the framework collects across all modules in dependency order and hands the
//! typed sets to consumers. It is keyed by Rust type, so a plugin can define its
//! own extension point (a trait or struct) and any other module can fill it with
//! no core change: the definer reads `items::<MyExtension>()`, contributors call
//! `add(my_extension)`.
//!
//! Framework-known surfaces (resources, permissions, ...) get typed, discoverable
//! sugar through extension traits defined in the layer that owns those types
//! (`laterite-admin`), so a wrong-type contribution is a compile error rather than
//! a silently invisible one; the generic `add`/`items` underneath is for open,
//! plugin-defined points.

use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::module::ModuleId;

/// How a contribution combines with earlier ones of the same type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContributeMode {
    /// Add alongside earlier contributions (the default).
    #[default]
    Append,
    /// Supersede all earlier contributions of the same type, for a singleton
    /// extension point where one contribution should win (a site overriding a
    /// default).
    Replace,
}

/// Priority and mode for a contribution.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContributeOpts {
    /// Higher sorts first among same-type contributions; default 0.
    pub priority: i32,
    pub mode: ContributeMode,
}

struct Entry {
    owner: ModuleId,
    priority: i32,
    mode: ContributeMode,
    item: Box<dyn Any>,
}

/// A borrowed contribution: the item plus the envelope the registry stamped.
pub struct Contribution<'a, T> {
    pub owner: ModuleId,
    pub priority: i32,
    pub mode: ContributeMode,
    pub item: &'a T,
}

/// Collects typed contributions from modules, keyed by Rust type.
#[derive(Default)]
pub struct Registry {
    current: Option<ModuleId>,
    by_type: HashMap<TypeId, Vec<Entry>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets which module is registering; its contributions are stamped with this
    /// owner. The framework calls this before each module's `register()`, so an
    /// author never passes an owner and cannot forge one.
    pub fn set_owner(&mut self, owner: ModuleId) {
        self.current = Some(owner);
    }

    /// Contributes `item`, keyed by its type, with the default envelope.
    pub fn add<T: 'static>(&mut self, item: T) {
        self.add_with(item, ContributeOpts::default());
    }

    /// Contributes `item` with an explicit priority and mode.
    pub fn add_with<T: 'static>(&mut self, item: T, opts: ContributeOpts) {
        let owner = self
            .current
            .expect("Registry::add called outside a module register() pass");
        self.by_type
            .entry(TypeId::of::<T>())
            .or_default()
            .push(Entry {
                owner,
                priority: opts.priority,
                mode: opts.mode,
                item: Box::new(item),
            });
    }

    /// The contributions of type `T`, resolved: a `Replace` supersedes everything
    /// before it, then higher priority sorts first, ties keeping dependency order.
    pub fn contributions<T: 'static>(&self) -> Vec<Contribution<'_, T>> {
        let Some(entries) = self.by_type.get(&TypeId::of::<T>()) else {
            return Vec::new();
        };
        let start = entries
            .iter()
            .rposition(|e| e.mode == ContributeMode::Replace)
            .unwrap_or(0);
        let mut resolved: Vec<&Entry> = entries[start..].iter().collect();
        // Stable sort, so equal priorities keep insertion (dependency) order.
        resolved.sort_by(|a, b| b.priority.cmp(&a.priority));
        resolved
            .into_iter()
            .map(|e| Contribution {
                owner: e.owner,
                priority: e.priority,
                mode: e.mode,
                item: e
                    .item
                    .downcast_ref::<T>()
                    .expect("keyed by TypeId::of::<T>"),
            })
            .collect()
    }

    /// The items of type `T`, in resolved order (envelope dropped).
    pub fn items<T: 'static>(&self) -> Vec<&T> {
        self.contributions::<T>()
            .into_iter()
            .map(|c| c.item)
            .collect()
    }

    /// Removes and returns the items of type `T` as owned values, resolved the
    /// same way as [`items`](Registry::items). For a one-time collection at boot;
    /// the type's contributions are consumed, so no `Clone` bound is needed.
    pub fn take<T: 'static>(&mut self) -> Vec<T> {
        let Some(entries) = self.by_type.remove(&TypeId::of::<T>()) else {
            return Vec::new();
        };
        let start = entries
            .iter()
            .rposition(|e| e.mode == ContributeMode::Replace)
            .unwrap_or(0);
        let mut kept: Vec<Entry> = entries.into_iter().skip(start).collect();
        // Stable sort, so equal priorities keep insertion (dependency) order.
        kept.sort_by(|a, b| b.priority.cmp(&a.priority));
        kept.into_iter()
            .map(|e| *e.item.downcast::<T>().expect("keyed by TypeId::of::<T>"))
            .collect()
    }

    /// Drops every contribution stamped with `owner`, across all types. Rolls back
    /// a module's partial contributions when its `register` fails, so a quarantined
    /// module leaves nothing behind.
    pub fn purge_owner(&mut self, owner: ModuleId) {
        for entries in self.by_type.values_mut() {
            entries.retain(|e| e.owner != owner);
        }
        self.by_type.retain(|_, entries| !entries.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: ModuleId = ModuleId::new("acme.a");
    const B: ModuleId = ModuleId::new("acme.b");

    #[derive(Debug, PartialEq)]
    struct Widget(&'static str);

    #[test]
    fn collects_in_order_and_stamps_owner() {
        let mut r = Registry::new();
        r.set_owner(A);
        r.add(Widget("one"));
        r.set_owner(B);
        r.add(Widget("two"));

        let cs = r.contributions::<Widget>();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].item, &Widget("one"));
        assert_eq!(cs[0].owner, A);
        assert_eq!(cs[1].owner, B);
        assert_eq!(r.items::<Widget>(), vec![&Widget("one"), &Widget("two")]);
    }

    #[test]
    fn higher_priority_sorts_first_ties_keep_order() {
        let mut r = Registry::new();
        r.set_owner(A);
        r.add(Widget("low"));
        r.add_with(
            Widget("high"),
            ContributeOpts {
                priority: 10,
                mode: ContributeMode::Append,
            },
        );
        r.add(Widget("low2"));
        assert_eq!(
            r.items::<Widget>(),
            vec![&Widget("high"), &Widget("low"), &Widget("low2")]
        );
    }

    #[test]
    fn replace_supersedes_earlier() {
        let mut r = Registry::new();
        r.set_owner(A);
        r.add(Widget("default"));
        r.set_owner(B);
        r.add_with(
            Widget("override"),
            ContributeOpts {
                priority: 0,
                mode: ContributeMode::Replace,
            },
        );
        assert_eq!(r.items::<Widget>(), vec![&Widget("override")]);
    }

    #[test]
    fn purge_owner_drops_only_that_owners_contributions() {
        let mut r = Registry::new();
        r.set_owner(A);
        r.add(Widget("a"));
        r.add(7u32);
        r.set_owner(B);
        r.add(Widget("b"));

        r.purge_owner(A);
        // B's widget survives; A's widget and A's u32 (its whole bucket) are gone.
        assert_eq!(r.items::<Widget>(), vec![&Widget("b")]);
        assert!(r.items::<u32>().is_empty());
    }

    #[test]
    fn distinct_types_are_independent() {
        let mut r = Registry::new();
        r.set_owner(A);
        r.add(Widget("w"));
        r.add(7u32);
        assert_eq!(r.items::<Widget>(), vec![&Widget("w")]);
        assert_eq!(r.items::<u32>(), vec![&7u32]);
    }
}
