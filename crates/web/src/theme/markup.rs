//! Markup a module adds to every theme engine: functions, filters, tests and
//! globals installed into the environment before the templates compile.
//!
//! A module implements [`Markup`] and registers a [`MarkupReg`]; the site and
//! the static build install every registration when they build an engine, in
//! registration order.
//!
//! ```no_run
//! use laterite_web::theme::markup::{Markup, MarkupReg};
//! use minijinja::Environment;
//! use std::sync::Arc;
//!
//! struct Money;
//!
//! impl Markup for Money {
//!     fn install(&self, env: &mut Environment<'static>) {
//!         env.add_filter("money", |minor: i64| format!("{}.{:02}", minor / 100, minor % 100));
//!     }
//! }
//!
//! # fn register(registry: &mut laterite_core::Registry) {
//! registry.add(MarkupReg::new("acme.money", Arc::new(Money)));
//! # }
//! ```

use minijinja::Environment;
use std::sync::Arc;

/// What a module installs into a theme engine.
pub trait Markup: Send + Sync + 'static {
    /// Adds the module's functions, filters, tests and globals to `env`.
    fn install(&self, env: &mut Environment<'static>);
}

/// A registered markup extension: a key naming it, and the extension.
#[non_exhaustive]
pub struct MarkupReg {
    /// A `vendor.name` key, for the boot log and a clash message.
    pub key: String,
    pub markup: Arc<dyn Markup>,
}

impl MarkupReg {
    pub fn new(key: impl Into<String>, markup: Arc<dyn Markup>) -> Self {
        Self {
            key: key.into(),
            markup,
        }
    }
}

/// Installs every registration into `env`, in order.
pub fn install_all(regs: &[Arc<dyn Markup>], env: &mut Environment<'static>) {
    for markup in regs {
        markup.install(env);
    }
}
