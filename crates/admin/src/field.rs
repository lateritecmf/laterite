//! The open field-type registry: the framework and plugins add form field types.
//!
//! A field descriptor ([`crate::form::FormField`]) is serde data (name, label,
//! type key, options, rules); a field type is behaviour keyed by that type.
//! Rendering splits so an override can present the same data the built-in does:
//! [`FieldType::view_model`] builds a serde [`FieldVm`], [`FieldType::render_default`]
//! is the Askama presenter over it, and an [`OverrideResolver`] (default
//! [`NoOverrides`]) may swap in a runtime template. Load/parse/save/assets/action
//! join as defaulted methods when a type first needs them (backward-compatible).

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use laterite_core::validation::Rule;
use serde::Serialize;

use crate::html::Markup;

/// A field's value, general enough for scalar and (later) structured types.
/// Serde so it rides in the view-model and a failed-validation re-render.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum FieldValue {
    Null,
    Text(String),
    Json(serde_json::Value),
}

impl FieldValue {
    /// The value as text for a scalar field (empty for null or non-text).
    pub fn as_text(&self) -> &str {
        match self {
            FieldValue::Text(s) => s,
            _ => "",
        }
    }
}

/// Per-type options, resolved once at boot from the descriptor's options blob
/// into a typed value cached for rendering. `text`/`textarea` carry none.
#[derive(Default)]
pub struct ResolvedOptions(Option<Arc<dyn Any + Send + Sync>>);

impl ResolvedOptions {
    /// No options (the default for optionless types).
    pub fn none() -> Self {
        Self(None)
    }
    /// Wraps a resolved typed options value.
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Some(Arc::new(value)))
    }
    /// Borrows the typed options, if present and of type `T`.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.0.as_deref().and_then(|a| a.downcast_ref::<T>())
    }
}

/// Whether the framework wraps a field in standard chrome (label, required
/// marker, error list) or the type renders bare (a hidden field, a heading).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chrome {
    Wrapped,
    Bare,
}

/// The resolved per-field state a field type renders from.
#[non_exhaustive]
pub struct FieldCx<'a> {
    /// The submitted/DOM field name (becomes a path with repeaters).
    pub name: &'a str,
    /// A DOM-safe id for the control and its label's `for`.
    pub id: &'a str,
    pub label: &'a str,
    pub value: &'a FieldValue,
    /// Derived from the merged (intrinsic + descriptor) rules.
    pub required: bool,
    pub opts: &'a ResolvedOptions,
}

/// The serialisable payload both the built-in template and an override render,
/// so an override never re-derives data. `data` is per-type data, never HTML.
#[derive(Debug, Clone, Serialize)]
pub struct FieldVm {
    pub view_key: String,
    pub name: String,
    pub id: String,
    pub label: String,
    pub required: bool,
    pub value: FieldValue,
    pub data: serde_json::Value,
}

/// A field type: behaviour registered once per string key.
pub trait FieldType: Send + Sync + 'static {
    /// The stable key for override resolution and default-template selection
    /// (bare like `text` for core; dotted `vendor.name` for plugins).
    fn view_key(&self) -> &'static str;

    /// Types the descriptor's raw options blob once at boot. A failure aborts
    /// boot naming the resource + field. Default: no options.
    fn resolve_options(&self, _raw: &serde_json::Value) -> Result<ResolvedOptions, OptionsError> {
        Ok(ResolvedOptions::none())
    }

    /// Rules this type always contributes, merged before the descriptor's own
    /// (e.g. an `email` type contributes [`Rule::Email`]).
    fn intrinsic_rules(&self, _opts: &ResolvedOptions) -> Vec<Rule> {
        Vec::new()
    }

    /// Builds the serialisable presentation payload. Pure, no IO.
    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm;

    /// The built-in presenter over the view-model (an Askama partial). Not
    /// called when an override wins.
    fn render_default(&self, vm: &FieldVm) -> Markup;

    /// Whether the framework wraps this field in standard chrome.
    fn chrome(&self) -> Chrome {
        Chrome::Wrapped
    }
}

/// A field type failed to type its options blob at boot.
#[derive(Debug, thiserror::Error)]
#[error("invalid field options: {0}")]
pub struct OptionsError(pub String);

/// The surface being rendered, for override resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Field,
    Column,
}

/// Where a render is happening, for override resolution (most-specific first:
/// `resource.field` then `view_key` then the compiled default).
pub struct OverrideScope<'a> {
    pub surface: Surface,
    pub view_key: &'a str,
    pub resource: Option<&'a str>,
    pub field: Option<&'a str>,
}

/// A runtime override failed to render.
#[derive(Debug, thiserror::Error)]
#[error("override render failed: {0}")]
pub struct OverrideError(pub String);

/// The seam a theme/CMS layer backs with a runtime template engine (MiniJinja)
/// to let users override a field's presentation from outside the plugin.
/// `laterite-admin` stays engine-agnostic; the default is [`NoOverrides`].
pub trait OverrideResolver: Send + Sync {
    /// An override for this view-model if one is registered for the scope, else
    /// `None` (use the default). The `String` is engine-autoescaped HTML.
    fn render_override(
        &self,
        scope: &OverrideScope<'_>,
        vm: &serde_json::Value,
    ) -> Option<Result<String, OverrideError>>;
}

/// The default resolver: no overrides, so the compiled path always runs at zero
/// cost. The theme layer injects a real resolver when it is built.
pub struct NoOverrides;

impl OverrideResolver for NoOverrides {
    fn render_override(
        &self,
        _scope: &OverrideScope<'_>,
        _vm: &serde_json::Value,
    ) -> Option<Result<String, OverrideError>> {
        None
    }
}

/// The field-type registry: type key to behaviour, built once at boot.
pub type FieldRegistry = HashMap<String, Arc<dyn FieldType>>;

/// Renders a field: an override if the resolver supplies one, else the compiled
/// default. The one place the two paths meet.
pub(crate) fn render_field(
    ft: &dyn FieldType,
    resolver: &dyn OverrideResolver,
    scope: &OverrideScope<'_>,
    cx: &FieldCx<'_>,
) -> Markup {
    let vm = ft.view_model(cx);
    match resolver.render_override(scope, &serde_json::to_value(&vm).unwrap_or_default()) {
        Some(Ok(html)) => Markup::from_override(html),
        // A failing override falls back to the default rather than blank the field.
        Some(Err(_)) | None => ft.render_default(&vm),
    }
}

/// The framework's built-in field types.
pub(crate) fn builtin_field_types() -> Vec<Arc<dyn FieldType>> {
    vec![Arc::new(TextField), Arc::new(TextareaField)]
}

/// The field registry seeded with the built-in types, keyed by [`FieldType::view_key`].
pub(crate) fn builtin_registry() -> FieldRegistry {
    builtin_field_types()
        .into_iter()
        .map(|ft| (ft.view_key().to_string(), ft))
        .collect()
}

#[derive(Template)]
#[template(path = "fields/text.html")]
struct TextTmpl<'a> {
    name: &'a str,
    id: &'a str,
    value: &'a str,
    required: bool,
}

/// A single-line text input.
pub(crate) struct TextField;

impl FieldType for TextField {
    fn view_key(&self) -> &'static str {
        "text"
    }
    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        scalar_vm("text", cx)
    }
    fn render_default(&self, vm: &FieldVm) -> Markup {
        Markup::from_template(&TextTmpl {
            name: &vm.name,
            id: &vm.id,
            value: vm.value.as_text(),
            required: vm.required,
        })
        .unwrap_or_default()
    }
}

#[derive(Template)]
#[template(path = "fields/textarea.html")]
struct TextareaTmpl<'a> {
    name: &'a str,
    id: &'a str,
    value: &'a str,
    required: bool,
}

/// A multi-line text input.
pub(crate) struct TextareaField;

impl FieldType for TextareaField {
    fn view_key(&self) -> &'static str {
        "textarea"
    }
    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        scalar_vm("textarea", cx)
    }
    fn render_default(&self, vm: &FieldVm) -> Markup {
        Markup::from_template(&TextareaTmpl {
            name: &vm.name,
            id: &vm.id,
            value: vm.value.as_text(),
            required: vm.required,
        })
        .unwrap_or_default()
    }
}

/// The view-model common to scalar text-like fields (no per-type `data`).
fn scalar_vm(view_key: &str, cx: &FieldCx<'_>) -> FieldVm {
    FieldVm {
        view_key: view_key.to_string(),
        name: cx.name.to_string(),
        id: cx.id.to_string(),
        label: cx.label.to_string(),
        required: cx.required,
        value: cx.value.clone(),
        data: serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx<'a>(name: &'a str, value: &'a FieldValue, opts: &'a ResolvedOptions) -> FieldCx<'a> {
        FieldCx {
            name,
            id: name,
            label: "Label",
            value,
            required: true,
            opts,
        }
    }

    #[test]
    fn text_renders_its_value_escaped() {
        let opts = ResolvedOptions::none();
        let value = FieldValue::Text("a<b>&\"c".to_string());
        let markup = render_field(
            &TextField,
            &NoOverrides,
            &scope(),
            &cx("title", &value, &opts),
        );
        let html = markup.as_str();
        assert!(html.contains(r#"name="title""#));
        assert!(html.contains("required"));
        // The value is HTML-escaped by the Askama partial: no raw tag injected,
        // and the angle/ampersand are escaped (named or numeric entity, Askama's
        // choice, so accept either).
        assert!(!html.contains("<b>"), "no raw tag injected: {html}");
        assert!(
            html.contains("&#60;") || html.contains("&lt;"),
            "angle escaped: {html}"
        );
        assert!(
            html.contains("&#38;") || html.contains("&amp;"),
            "ampersand escaped: {html}"
        );
    }

    #[test]
    fn textarea_uses_its_own_control() {
        let opts = ResolvedOptions::none();
        let value = FieldValue::Text("body".to_string());
        let markup = render_field(
            &TextareaField,
            &NoOverrides,
            &scope(),
            &cx("body", &value, &opts),
        );
        assert!(markup.as_str().contains("<textarea"));
    }

    #[test]
    fn no_overrides_uses_the_compiled_default() {
        // A resolver that never overrides yields the built-in markup unchanged.
        let opts = ResolvedOptions::none();
        let value = FieldValue::Text("x".to_string());
        let vm = TextField.view_model(&cx("t", &value, &opts));
        let direct = TextField.render_default(&vm);
        let routed = render_field(&TextField, &NoOverrides, &scope(), &cx("t", &value, &opts));
        assert_eq!(direct.as_str(), routed.as_str());
    }

    fn scope<'a>() -> OverrideScope<'a> {
        OverrideScope {
            surface: Surface::Field,
            view_key: "text",
            resource: None,
            field: None,
        }
    }
}
