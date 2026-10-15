//! Components: compiled units a page or layout attaches, each producing data
//! a template reads under the alias it was attached as.
//!
//! A module implements [`Component`] and registers a [`ComponentReg`]. A page
//! or layout attaches one in its settings under `components`, keyed by alias,
//! naming the component and its properties; a property written as
//! `{{ :name }}` takes the URL parameter of that name at request time:
//!
//! ```json
//! {
//!   "url": "/blog/:slug",
//!   "components": {
//!     "post": { "component": "acme.post", "slug": "{{ :slug }}" },
//!     "latest": { "component": "acme.posts", "limit": 5 }
//!   }
//! }
//! ```
//!
//! The site runs every attached component before the page renders, in the
//! order attached (the layout's first, then the page's, a page alias winning
//! over a layout's), and exposes each result as `{{ post }}` and `{{ latest }}`
//! plus any variables a component injects into the page scope. A save checks
//! the attachment: the component must be registered and every property one
//! it declares, of the kind declared.

use super::router::Params;
use super::ThemeError;
use laterite_core::Db;
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a component property holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyKind {
    Text,
    Number,
    Bool,
}

/// A property a component declares, for the editor and the save-time check.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PropertyDef {
    pub name: String,
    pub label: String,
    pub kind: PropertyKind,
    pub required: bool,
    /// The value used when the attachment leaves the property out.
    pub default: Option<serde_json::Value>,
}

impl PropertyDef {
    pub fn new(name: impl Into<String>, label: impl Into<String>, kind: PropertyKind) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            kind,
            required: false,
            default: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn default(mut self, value: impl Into<serde_json::Value>) -> Self {
        self.default = Some(value.into());
        self
    }
}

/// What a component runs with.
pub struct ComponentContext<'a> {
    pub db: &'a Db,
    /// The properties as attached, with `{{ :name }}` resolved and defaults filled.
    pub properties: &'a serde_json::Map<String, serde_json::Value>,
    /// The URL's parameters.
    pub params: &'a Params,
    /// The request path.
    pub path: &'a str,
    /// The theme being rendered.
    pub theme: &'a str,
    /// Whether an operator is previewing.
    pub preview: bool,
}

/// What a component produced.
#[derive(Debug, Clone, Default)]
pub struct ComponentOutput {
    /// Read as the alias the component was attached under.
    pub data: serde_json::Value,
    /// Variables placed in the page scope under their own names.
    pub inject: serde_json::Map<String, serde_json::Value>,
}

impl ComponentOutput {
    /// Data under the alias and nothing injected.
    pub fn data(value: impl Into<serde_json::Value>) -> Self {
        Self {
            data: value.into(),
            inject: Default::default(),
        }
    }

    /// Adds a variable to the page scope.
    pub fn inject(mut self, name: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.inject.insert(name.into(), value.into());
        self
    }
}

/// A compiled unit a page attaches.
#[laterite_core::strata::async_trait]
pub trait Component: Send + Sync + 'static {
    /// The properties an attachment may set.
    fn properties(&self) -> Vec<PropertyDef> {
        Vec::new()
    }

    /// The component's default markup, rendered by `component("alias")` when
    /// the theme has no `partials/<alias>/default.htm` of its own. It reads
    /// the component's data as `component` and as the alias.
    fn markup(&self) -> Option<&'static str> {
        None
    }

    /// Produces the data for one request.
    async fn run(&self, cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError>;
}

/// A registered component: its `vendor.name` code, a label for the editor,
/// and the unit.
#[non_exhaustive]
pub struct ComponentReg {
    pub code: String,
    pub label: String,
    pub component: Arc<dyn Component>,
}

impl ComponentReg {
    pub fn new(
        code: impl Into<String>,
        label: impl Into<String>,
        component: Arc<dyn Component>,
    ) -> Self {
        Self {
            code: code.into(),
            label: label.into(),
            component,
        }
    }
}

/// The environment name of a component's default markup.
pub fn template_name(code: &str) -> String {
    format!("components/{code}.htm")
}

/// The context key holding each alias's component code for `component()`.
pub const ALIASES_KEY: &str = "__components";

/// One attachment: an alias, a component code and its raw properties.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub alias: String,
    pub code: String,
    pub properties: serde_json::Map<String, serde_json::Value>,
}

/// The attachments in a settings bag's `components`, in the order written.
pub fn attached(bag: &serde_json::Value) -> Result<Vec<Attachment>, ThemeError> {
    let Some(map) = bag.get("components") else {
        return Ok(Vec::new());
    };
    let Some(map) = map.as_object() else {
        return Err(ThemeError::Page(
            "components must be an object keyed by alias".into(),
        ));
    };
    let mut out = Vec::new();
    for (alias, spec) in map {
        if alias.is_empty() || !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(ThemeError::Page(format!(
                "{alias:?} is not a component alias: letters, digits and underscores"
            )));
        }
        let Some(spec) = spec.as_object() else {
            return Err(ThemeError::Page(format!(
                "the component {alias:?} must be an object with a component code"
            )));
        };
        let Some(code) = spec.get("component").and_then(|c| c.as_str()) else {
            return Err(ThemeError::Page(format!(
                "the component {alias:?} names no component"
            )));
        };
        let mut properties = spec.clone();
        properties.remove("component");
        out.push(Attachment {
            alias: alias.clone(),
            code: code.to_string(),
            properties,
        });
    }
    Ok(out)
}

/// Checks a bag's attachments against the registered components: each named
/// one exists, each property is declared and of its kind (a `{{ :name }}`
/// reference passes for any kind), and every required one is set.
pub fn check(bag: &serde_json::Value, regs: &[&ComponentReg]) -> Result<(), ThemeError> {
    for a in attached(bag)? {
        let Some(reg) = regs.iter().find(|r| r.code == a.code) else {
            return Err(ThemeError::Page(format!(
                "no component {:?} for {:?}",
                a.code, a.alias
            )));
        };
        let defs = reg.component.properties();
        for (name, value) in &a.properties {
            let Some(def) = defs.iter().find(|d| d.name == *name) else {
                return Err(ThemeError::Page(format!(
                    "{:?} has no property {name:?}",
                    a.code
                )));
            };
            if external(value).is_some() {
                continue;
            }
            let ok = match def.kind {
                PropertyKind::Text => value.is_string(),
                PropertyKind::Number => value.is_number(),
                PropertyKind::Bool => value.is_boolean(),
            };
            if !ok {
                return Err(ThemeError::Page(format!(
                    "the property {name:?} of {:?} takes a {}",
                    a.code,
                    match def.kind {
                        PropertyKind::Text => "text",
                        PropertyKind::Number => "number",
                        PropertyKind::Bool => "true or false",
                    }
                )));
            }
        }
        for def in defs.iter().filter(|d| d.required && d.default.is_none()) {
            if !a.properties.contains_key(&def.name) {
                return Err(ThemeError::Page(format!(
                    "{:?} needs the property {:?}",
                    a.code, def.name
                )));
            }
        }
    }
    Ok(())
}

/// The URL parameter a property value refers to, for `{{ :name }}`.
fn external(value: &serde_json::Value) -> Option<&str> {
    let text = value.as_str()?.trim();
    let inner = text.strip_prefix("{{")?.strip_suffix("}}")?.trim();
    inner.strip_prefix(':').map(str::trim)
}

/// The properties a component runs with: the attachment's, with `{{ :name }}`
/// resolved from `params` and the declared defaults filled.
pub fn resolve(
    a: &Attachment,
    defs: &[PropertyDef],
    params: &Params,
) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for def in defs {
        if let Some(default) = &def.default {
            out.insert(def.name.clone(), default.clone());
        }
    }
    for (name, value) in &a.properties {
        let value = match external(value) {
            Some(param) => params
                .get(param)
                .cloned()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
            None => value.clone(),
        };
        out.insert(name.clone(), value);
    }
    out
}

/// What running the attachments of a page gave: a value per alias, then the
/// injected variables, as pairs for the render context.
pub struct Ran {
    pub pairs: Vec<(String, serde_json::Value)>,
}

/// Adds every registered component's default markup to `env`, so
/// `component()` can fall back to it.
pub fn install_markup(regs: &[&ComponentReg], env: &mut minijinja::Environment<'static>) {
    for reg in regs {
        if let Some(markup) = reg.component.markup() {
            // A default that does not compile is a module bug, logged and skipped.
            if let Err(e) = env.add_template_owned(template_name(&reg.code), markup.to_string()) {
                tracing::error!(component = %reg.code, error = %e, "a component's default markup does not compile");
            }
        }
    }
}

/// Runs every attachment in `bags` (a layout's, then a page's; a later alias
/// replaces an earlier one), returning the context pairs.
pub async fn run_all(
    bags: &[&serde_json::Value],
    regs: &[&ComponentReg],
    db: &Db,
    params: &Params,
    path: &str,
    theme: &str,
    preview: bool,
) -> Result<Ran, ThemeError> {
    let mut by_alias: BTreeMap<String, Attachment> = BTreeMap::new();
    for bag in bags {
        for a in attached(bag)? {
            by_alias.insert(a.alias.clone(), a);
        }
    }
    let mut pairs: Vec<(String, serde_json::Value)> = Vec::new();
    let mut injected: Vec<(String, serde_json::Value)> = Vec::new();
    for a in by_alias.values() {
        let Some(reg) = regs.iter().find(|r| r.code == a.code) else {
            return Err(ThemeError::Page(format!(
                "no component {:?} for {:?}",
                a.code, a.alias
            )));
        };
        let properties = resolve(a, &reg.component.properties(), params);
        let cx = ComponentContext {
            db,
            properties: &properties,
            params,
            path,
            theme,
            preview,
        };
        let output = reg.component.run(&cx).await?;
        pairs.push((a.alias.clone(), output.data));
        injected.extend(output.inject);
    }
    // An alias wins over an injected variable of the same name.
    for (name, value) in injected {
        if !pairs.iter().any(|(n, _)| *n == name) {
            pairs.push((name, value));
        }
    }
    let aliases: serde_json::Map<String, serde_json::Value> = by_alias
        .values()
        .map(|a| (a.alias.clone(), serde_json::Value::String(a.code.clone())))
        .collect();
    pairs.push((ALIASES_KEY.to_string(), serde_json::Value::Object(aliases)));
    Ok(Ran { pairs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachments_parse_and_external_properties_resolve() {
        let bag = serde_json::json!({
            "url": "/blog/:slug",
            "components": {
                "post": {"component": "acme.post", "slug": "{{ :slug }}", "full": true},
                "latest": {"component": "acme.posts", "limit": 5}
            }
        });
        let list = attached(&bag).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].alias, "post");
        assert_eq!(list[1].code, "acme.posts");
        let defs = vec![
            PropertyDef::new("slug", "Slug", PropertyKind::Text).required(),
            PropertyDef::new("full", "Full", PropertyKind::Bool).default(false),
            PropertyDef::new("limit", "Limit", PropertyKind::Number).default(10),
        ];
        let params: Params = [("slug".to_string(), "hello".to_string())].into();
        let resolved = resolve(&list[0], &defs, &params);
        assert_eq!(resolved["slug"], "hello");
        assert_eq!(resolved["full"], true);
        assert_eq!(resolved["limit"], 10);
        assert!(attached(&serde_json::json!({"components": {"bad alias": {}}})).is_err());
        assert!(attached(&serde_json::json!({"components": {"a": {"x": 1}}})).is_err());
        assert!(attached(&serde_json::json!({"components": []})).is_err());
        assert!(attached(&serde_json::json!({"url": "/"}))
            .unwrap()
            .is_empty());
    }
}
