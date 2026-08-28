//! Module registration.
//!
//! Both framework crates and application feature crates (plugins) expose a
//! [`Module`]: a unit with a stable id, its own migrations, and the ids of the
//! modules it depends on. The binary registers them in a [`ModuleRegistry`],
//! which orders their migrations so a module's dependencies migrate first, and
//! answers whether a given module is present. Registration surfaces grow as the
//! framework does (navigation, permissions, settings, and event listeners arrive
//! with their crates); the trait stays minimal until a caller needs more.

use std::collections::HashMap;

use crate::error::{CoreError, CoreResult};
use crate::migration::MigrationSet;

/// A registerable unit: a framework crate or an application feature/plugin.
pub trait Module: Send + Sync + 'static {
    /// Stable identity: a lowercase `vendor.package` code, e.g. `"acme.blog"`.
    /// It namespaces this module's applied-migration history and is the target of
    /// other modules' [`depends_on`](Module::depends_on),
    /// so keep it stable and never adopt another module's. Must equal the
    /// `module_id` of the set returned by [`migrations`](Module::migrations).
    fn id(&self) -> &'static str;

    /// This module's migrations, in apply order. Defaults to none, for a
    /// code-only module.
    fn migrations(&self) -> MigrationSet {
        MigrationSet::new(self.id(), Vec::new())
    }

    /// Ids of modules whose migrations must run before this module's, so
    /// cross-module foreign keys resolve. Defaults to none.
    fn depends_on(&self) -> &'static [&'static str] {
        &[]
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

    /// Registers a module. Registration order breaks ties in the migration
    /// ordering, so foundational modules register first.
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
    pub fn has(&self, id: &str) -> bool {
        self.modules.iter().any(|m| m.id() == id)
    }

    /// The registered module with this id, if any.
    pub fn get(&self, id: &str) -> Option<&dyn Module> {
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

    /// Every module's [`MigrationSet`], ordered so a module's dependencies come
    /// before it (topological order); registration order breaks ties, keeping
    /// the result stable. Feed the result to [`migration::run`](crate::migration::run).
    ///
    /// Errors on a duplicate id, a dependency on an unregistered module, or a
    /// dependency cycle.
    pub fn ordered_migration_sets(&self) -> CoreResult<Vec<MigrationSet>> {
        let mut by_id: HashMap<&str, &dyn Module> = HashMap::with_capacity(self.modules.len());
        for m in &self.modules {
            if by_id.insert(m.id(), m.as_ref()).is_some() {
                return Err(CoreError::DuplicateModule {
                    module: m.id().to_string(),
                });
            }
        }
        for m in &self.modules {
            for dep in m.depends_on() {
                if !by_id.contains_key(dep) {
                    return Err(CoreError::UnknownModuleDependency {
                        module: m.id().to_string(),
                        dependency: dep.to_string(),
                    });
                }
            }
        }

        let mut state: HashMap<&str, Visit> = HashMap::with_capacity(self.modules.len());
        let mut order: Vec<&str> = Vec::with_capacity(self.modules.len());
        for m in &self.modules {
            visit(m.id(), &by_id, &mut state, &mut order)?;
        }
        Ok(order.into_iter().map(|id| by_id[id].migrations()).collect())
    }
}

#[derive(Clone, Copy)]
enum Visit {
    Active,
    Done,
}

/// Depth-first post-order visit; `Active` marks the current stack, so meeting it
/// again is a cycle.
fn visit<'a>(
    id: &'a str,
    by_id: &HashMap<&'a str, &'a dyn Module>,
    state: &mut HashMap<&'a str, Visit>,
    order: &mut Vec<&'a str>,
) -> CoreResult<()> {
    match state.get(id) {
        Some(Visit::Done) => return Ok(()),
        Some(Visit::Active) => {
            return Err(CoreError::ModuleDependencyCycle {
                module: id.to_string(),
            })
        }
        None => {}
    }
    state.insert(id, Visit::Active);
    for dep in by_id[id].depends_on() {
        visit(dep, by_id, state, order)?;
    }
    state.insert(id, Visit::Done);
    order.push(id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test module: fixed id and dependencies, no migrations.
    struct Mod(&'static str, &'static [&'static str]);

    impl Module for Mod {
        fn id(&self) -> &'static str {
            self.0
        }
        fn depends_on(&self) -> &'static [&'static str] {
            self.1
        }
    }

    fn order_of(reg: &ModuleRegistry) -> Vec<String> {
        reg.ordered_migration_sets()
            .unwrap()
            .into_iter()
            .map(|s| s.module_id.to_string())
            .collect()
    }

    #[test]
    fn dependencies_come_before_dependents() {
        let mut reg = ModuleRegistry::new();
        // Registered dependents-first on purpose: ordering must still put deps first.
        reg.register(Mod("app.location", &["core.location", "core.geo"]))
            .register(Mod("core.geo", &[]))
            .register(Mod("core.location", &[]));
        let order = order_of(&reg);
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("core.location") < pos("app.location"));
        assert!(pos("core.geo") < pos("app.location"));
    }

    #[test]
    fn registration_order_breaks_ties() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod("a", &[]))
            .register(Mod("b", &[]))
            .register(Mod("c", &[]));
        assert_eq!(order_of(&reg), ["a", "b", "c"]);
    }

    #[test]
    fn has_and_get_find_by_id() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod("core.geo", &[]));
        assert!(reg.has("core.geo"));
        assert!(!reg.has("core.location"));
        assert_eq!(reg.get("core.geo").unwrap().id(), "core.geo");
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn unknown_dependency_errors() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod("app.location", &["core.location"]));
        assert!(matches!(
            reg.ordered_migration_sets(),
            Err(CoreError::UnknownModuleDependency { .. })
        ));
    }

    #[test]
    fn cycle_errors() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod("a", &["b"])).register(Mod("b", &["a"]));
        assert!(matches!(
            reg.ordered_migration_sets(),
            Err(CoreError::ModuleDependencyCycle { .. })
        ));
    }

    #[test]
    fn duplicate_id_errors() {
        let mut reg = ModuleRegistry::new();
        reg.register(Mod("a", &[])).register(Mod("a", &[]));
        assert!(matches!(
            reg.ordered_migration_sets(),
            Err(CoreError::DuplicateModule { .. })
        ));
    }
}
