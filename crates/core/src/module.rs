//! Module registration.
//!
//! Both framework crates and application feature crates (plugins) expose a
//! [`Module`]: a unit with a stable [`ModuleId`], its own migrations, the modules
//! and database capabilities it requires, and the contributions it registers. The
//! binary registers them in a [`ModuleRegistry`], which orders everything so a
//! module's dependencies come first, and answers whether a module is present.

use std::collections::HashMap;

use crate::error::{CoreError, CoreResult};
use crate::migration::MigrationSet;
use crate::registry::Registry;

/// A module's stable identity: a lowercase `vendor.package` code (e.g.
/// `ModuleId::new("acme.blog")`), declared once and never changed. A newtype so an
/// id is never confused with an arbitrary string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(&'static str);

impl ModuleId {
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// A named database capability a module requires or uses optionally (currently a
/// PostgreSQL extension, e.g. `Capability::new("postgis")`). A newtype for the
/// same reason as [`ModuleId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Capability(&'static str);

impl Capability {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

/// A registerable unit: a framework crate or an application feature/plugin.
pub trait Module: Send + Sync + 'static {
    /// Stable identity, a lowercase `vendor.package` code. Namespaces this
    /// module's applied-migration history and is the target of other modules'
    /// [`requires`](Module::requires). Its string must equal the `module_id` of
    /// the set returned by [`migrations`](Module::migrations).
    fn id(&self) -> ModuleId;

    /// This module's migrations, in apply order. Defaults to none, for a
    /// code-only module.
    fn migrations(&self) -> MigrationSet {
        MigrationSet::new(self.id().as_str(), Vec::new())
    }

    /// Ids of modules whose migrations and registration must run before this
    /// module's, so cross-module foreign keys and contributions resolve. Defaults
    /// to none.
    fn requires(&self) -> &'static [ModuleId] {
        &[]
    }

    /// Database capabilities this module cannot work without. The application
    /// refuses to boot if one is unavailable, with a clear error naming the
    /// module and capability, instead of failing deep inside a migration.
    fn requires_db_capabilities(&self) -> &'static [Capability] {
        &[]
    }

    /// Database capabilities this module uses when present and does without when
    /// absent. Query the boot-time [`CapabilitySet`](crate::capabilities::CapabilitySet)
    /// to gate the enhanced path (for example trigram fuzzy search).
    fn optional_db_capabilities(&self) -> &'static [Capability] {
        &[]
    }

    /// Contributes this module's items (resources, permissions, nav, settings,
    /// field types, and any plugin-defined extension items) to the [`Registry`].
    /// Runs in [`requires`](Module::requires) order, after migrations. Default:
    /// contributes nothing.
    fn register(&self, registry: &mut Registry) {
        let _ = registry;
    }
}

/// Ordered collection of registered modules.
#[derive(Default)]
pub struct ModuleRegistry {
    modules: Vec<Box<dyn Module>>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a module. Registration order breaks ties in the ordering, so
    /// foundational modules register first.
    pub fn register(&mut self, module: impl Module) -> &mut Self {
        self.modules.push(Box::new(module));
        self
    }

    /// Registers an already-boxed module, for callers assembling a
    /// `Vec<Box<dyn Module>>` (the built-in set, an app's plugins).
    pub fn register_boxed(&mut self, module: Box<dyn Module>) -> &mut Self {
        self.modules.push(module);
        self
    }

    /// Whether a module with this id is registered.
    pub fn has(&self, id: ModuleId) -> bool {
        self.modules.iter().any(|m| m.id() == id)
    }

    /// The registered module with this id, if any.
    pub fn get(&self, id: ModuleId) -> Option<&dyn Module> {
        self.modules
            .iter()
            .map(|m| m.as_ref())
            .find(|m| m.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Module> + '_ {
        self.modules.iter().map(|m| m.as_ref())
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// The registered modules in dependency order (a module after every module it
    /// [`requires`](Module::requires)); registration order breaks ties. Errors on
    /// a duplicate id, a required module that is not registered, or a cycle. This
    /// is the canonical order for both migrations and registration.
    pub fn ordered(&self) -> CoreResult<Vec<&dyn Module>> {
        let mut by_id: HashMap<ModuleId, &dyn Module> = HashMap::with_capacity(self.modules.len());
        for m in &self.modules {
            if by_id.insert(m.id(), m.as_ref()).is_some() {
                return Err(CoreError::DuplicateModule {
                    module: m.id().to_string(),
                });
            }
        }
        for m in &self.modules {
            for dep in m.requires() {
                if !by_id.contains_key(dep) {
                    return Err(CoreError::UnknownModuleDependency {
                        module: m.id().to_string(),
                        dependency: dep.to_string(),
                    });
                }
            }
        }

        let mut state: HashMap<ModuleId, Visit> = HashMap::with_capacity(self.modules.len());
        let mut order: Vec<ModuleId> = Vec::with_capacity(self.modules.len());
        for m in &self.modules {
            visit(m.id(), &by_id, &mut state, &mut order)?;
        }
        Ok(order.into_iter().map(|id| by_id[&id]).collect())
    }

    /// Every module's [`MigrationSet`] in dependency order. Feed the result to
    /// [`migration::run`](crate::migration::run).
    pub fn ordered_migration_sets(&self) -> CoreResult<Vec<MigrationSet>> {
        Ok(self
            .ordered()?
            .into_iter()
            .map(|m| m.migrations())
            .collect())
    }
}

#[derive(Clone, Copy)]
enum Visit {
    Active,
    Done,
}

/// Depth-first post-order visit; `Active` marks the current stack, so meeting it
/// again is a cycle.
fn visit(
    id: ModuleId,
    by_id: &HashMap<ModuleId, &dyn Module>,
    state: &mut HashMap<ModuleId, Visit>,
    order: &mut Vec<ModuleId>,
) -> CoreResult<()> {
    match state.get(&id) {
        Some(Visit::Done) => return Ok(()),
        Some(Visit::Active) => {
            return Err(CoreError::ModuleDependencyCycle {
                module: id.to_string(),
            })
        }
        None => {}
    }
    state.insert(id, Visit::Active);
    for dep in by_id[&id].requires() {
        visit(*dep, by_id, state, order)?;
    }
    state.insert(id, Visit::Done);
    order.push(id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test module: fixed id and required-module ids, no migrations.
    struct Mod(ModuleId, &'static [ModuleId]);

    impl Module for Mod {
        fn id(&self) -> ModuleId {
            self.0
        }
        fn requires(&self) -> &'static [ModuleId] {
            self.1
        }
    }

    const BLOG: ModuleId = ModuleId::new("acme.blog");
    const USER: ModuleId = ModuleId::new("acme.user");
    const MEDIA: ModuleId = ModuleId::new("acme.media");

    fn order_of(reg: &ModuleRegistry) -> Vec<&'static str> {
        reg.ordered()
            .unwrap()
            .into_iter()
            .map(|m| m.id().as_str())
            .collect()
    }

    #[test]
    fn dependencies_come_before_dependents() {
        let mut reg = ModuleRegistry::new();
        // Registered dependents-first on purpose: ordering must still put deps first.
        reg.register(Mod(BLOG, &[USER, MEDIA]))
            .register(Mod(MEDIA, &[]))
            .register(Mod(USER, &[]));
        let order = order_of(&reg);
        let pos = |id: &str| order.iter().position(|x| *x == id).unwrap();
        assert!(pos("acme.user") < pos("acme.blog"));
        assert!(pos("acme.media") < pos("acme.blog"));
    }

    #[test]
    fn registration_order_breaks_ties() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod(ModuleId::new("a"), &[]))
            .register(Mod(ModuleId::new("b"), &[]))
            .register(Mod(ModuleId::new("c"), &[]));
        assert_eq!(order_of(&reg), ["a", "b", "c"]);
    }

    #[test]
    fn has_and_get_find_by_id() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod(MEDIA, &[]));
        assert!(reg.has(MEDIA));
        assert!(!reg.has(USER));
        assert_eq!(reg.get(MEDIA).unwrap().id(), MEDIA);
        assert!(reg.get(ModuleId::new("missing")).is_none());
    }

    #[test]
    fn unknown_dependency_errors() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod(BLOG, &[USER]));
        assert!(matches!(
            reg.ordered(),
            Err(CoreError::UnknownModuleDependency { .. })
        ));
    }

    #[test]
    fn cycle_errors() {
        // Required-module arrays are static, so declare them as consts (a user
        // const fn's result is not promoted to `'static`).
        const A_REQS: &[ModuleId] = &[ModuleId::new("acme.b")];
        const B_REQS: &[ModuleId] = &[ModuleId::new("acme.a")];
        let mut reg = ModuleRegistry::new();
        reg.register(Mod(ModuleId::new("acme.a"), A_REQS))
            .register(Mod(ModuleId::new("acme.b"), B_REQS));
        assert!(matches!(
            reg.ordered(),
            Err(CoreError::ModuleDependencyCycle { .. })
        ));
    }

    #[test]
    fn duplicate_id_errors() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod(ModuleId::new("a"), &[]))
            .register(Mod(ModuleId::new("a"), &[]));
        assert!(matches!(
            reg.ordered(),
            Err(CoreError::DuplicateModule { .. })
        ));
    }
}
