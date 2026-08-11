//! Module registration.
//!
//! Both framework crates and application feature crates expose a [`Module`];
//! the binary assembles the registry at startup. Registration surfaces grow
//! as the framework does (navigation, permissions, settings, and event
//! listeners arrive with their crates); the trait stays minimal until a
//! caller needs more.

/// A registerable unit of the application.
pub trait Module: Send + Sync + 'static {
    /// Stable identifier, e.g. `"auth"`.
    fn name(&self) -> &'static str;
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

    /// Registers a module. Order is preserved and meaningful: foundational
    /// modules register first.
    pub fn register(&mut self, module: impl Module) -> &mut Self {
        self.modules.push(Box::new(module));
        self
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
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;

    impl Module for A {
        fn name(&self) -> &'static str {
            "a"
        }
    }

    impl Module for B {
        fn name(&self) -> &'static str {
            "b"
        }
    }

    #[test]
    fn preserves_registration_order() {
        let mut registry = ModuleRegistry::new();
        registry.register(A).register(B);
        let names: Vec<_> = registry.iter().map(|m| m.name()).collect();
        assert_eq!(names, ["a", "b"]);
    }
}
