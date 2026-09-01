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
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use askama::Template;
use laterite_core::validation::Rule;
use serde::{Deserialize, Serialize};

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
/// into a typed value cached for rendering (the text field resolves its input
/// variant, select its option list; textarea carries none).
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
    /// (e.g. the text field's `email` input contributes [`Rule::Email`]).
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

/// The framework's built-in field types. The text field is constructed with the
/// built-in input-type registry it delegates to.
pub(crate) fn builtin_field_types() -> Vec<Arc<dyn FieldType>> {
    let inputs = Arc::new(builtin_input_registry());
    vec![
        Arc::new(TextField::new(inputs)),
        Arc::new(TextareaField),
        Arc::new(SelectField),
    ]
}

/// The field registry seeded with the built-in types, keyed by [`FieldType::view_key`].
pub(crate) fn builtin_registry() -> FieldRegistry {
    builtin_field_types()
        .into_iter()
        .map(|ft| (ft.view_key().to_string(), ft))
        .collect()
}

/// Where an adornment sits relative to the input.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Leading,
    Trailing,
}

/// A control placed beside the input (a copy button, a currency symbol): data in
/// the view-model, never HTML, so an override re-presents it. The text template
/// renders it into the input group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adornment {
    pub placement: Placement,
    /// `true` renders a `<button>` (an action); `false` an inert affix `<span>`.
    pub button: bool,
    /// Visible text and accessible label.
    pub label: String,
    /// The `data-lat-widget` island wiring a button (bare like `copy` for core,
    /// dotted `vendor.name` for a plugin). `None` for an inert affix.
    pub widget: Option<String>,
}

/// An input variant of the text field: its HTML `type`, the rules it contributes,
/// and how it parameterises the one text control (extra attributes, adjacent
/// adornments). The extension point a plugin builds on (a currency input, say)
/// instead of reimplementing a field. An input parameterises the single text
/// control; anything needing different control markup is a [`FieldType`].
pub trait InputType: Send + Sync + 'static {
    /// The registry key naming this input (`text`, `email`, `url`, `currency`).
    fn key(&self) -> &'static str;
    /// The HTML `type` attribute.
    fn html_type(&self) -> &'static str {
        "text"
    }
    /// Types this input's own keys from the field's options blob (the same blob
    /// the field type reads) once at boot. Default: no options.
    fn resolve_options(&self, _raw: &serde_json::Value) -> Result<ResolvedOptions, OptionsError> {
        Ok(ResolvedOptions::none())
    }
    /// Rules this input contributes (an `email` input adds [`Rule::Email`]).
    fn rules(&self, _opts: &ResolvedOptions) -> Vec<Rule> {
        Vec::new()
    }
    /// Extra `<input>` attributes (a number input's `min`/`max`/`step`). Names are
    /// validated at boot and may not collide with the ones the template owns.
    fn attributes(&self, _opts: &ResolvedOptions) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    /// Controls placed beside the input (a copy button, a currency symbol).
    fn adornments(&self, _opts: &ResolvedOptions) -> Vec<Adornment> {
        Vec::new()
    }
}

/// The registry the text field resolves its `input` option against.
pub type InputRegistry = HashMap<String, Arc<dyn InputType>>;

struct TextInput;
impl InputType for TextInput {
    fn key(&self) -> &'static str {
        "text"
    }
}

struct EmailInput;
impl InputType for EmailInput {
    fn key(&self) -> &'static str {
        "email"
    }
    fn html_type(&self) -> &'static str {
        "email"
    }
    fn rules(&self, _opts: &ResolvedOptions) -> Vec<Rule> {
        vec![Rule::Email]
    }
}

struct TelInput;
impl InputType for TelInput {
    fn key(&self) -> &'static str {
        "tel"
    }
    fn html_type(&self) -> &'static str {
        "tel"
    }
}

/// A number input's optional bounds and step, emitted as `<input>` attributes.
#[derive(Default, Deserialize)]
struct NumberOptions {
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    step: Option<f64>,
}

struct NumberInput;
impl InputType for NumberInput {
    fn key(&self) -> &'static str {
        "number"
    }
    fn html_type(&self) -> &'static str {
        "number"
    }
    fn resolve_options(&self, raw: &serde_json::Value) -> Result<ResolvedOptions, OptionsError> {
        let opts: NumberOptions = if raw.is_null() {
            NumberOptions::default()
        } else {
            serde_json::from_value(raw.clone()).map_err(|e| OptionsError(e.to_string()))?
        };
        Ok(ResolvedOptions::new(opts))
    }
    fn rules(&self, _opts: &ResolvedOptions) -> Vec<Rule> {
        vec![Rule::Numeric]
    }
    fn attributes(&self, opts: &ResolvedOptions) -> BTreeMap<String, String> {
        let mut attrs = BTreeMap::new();
        if let Some(o) = opts.get::<NumberOptions>() {
            if let Some(min) = o.min {
                attrs.insert("min".to_string(), min.to_string());
            }
            if let Some(max) = o.max {
                attrs.insert("max".to_string(), max.to_string());
            }
            if let Some(step) = o.step {
                attrs.insert("step".to_string(), step.to_string());
            }
        }
        attrs
    }
}

pub(crate) fn builtin_input_types() -> Vec<Arc<dyn InputType>> {
    vec![
        Arc::new(TextInput),
        Arc::new(EmailInput),
        Arc::new(TelInput),
        Arc::new(NumberInput),
    ]
}

/// The input-type registry seeded with the built-in inputs.
pub(crate) fn builtin_input_registry() -> InputRegistry {
    builtin_input_types()
        .into_iter()
        .map(|i| (i.key().to_string(), i))
        .collect()
}

/// Attribute names the text template emits itself; an input may not re-declare
/// them (HTML resolves a duplicate to the first, silently dropping the input's).
const RESERVED_ATTRS: [&str; 6] = ["type", "id", "name", "value", "required", "class"];

/// Validates an input's contributed attribute and widget names at boot. Names
/// come from plugin code, so a bad one is a wiring bug caught at boot, not user
/// input: an attribute name is lowercase kebab and unreserved; a widget name may
/// carry a dotted plugin namespace.
fn validate_contributions(
    attrs: &BTreeMap<String, String>,
    adornments: &[Adornment],
) -> Result<(), OptionsError> {
    for name in attrs.keys() {
        if !is_name(name, false) {
            return Err(OptionsError(format!("invalid attribute name `{name}`")));
        }
        if RESERVED_ATTRS.contains(&name.as_str()) {
            return Err(OptionsError(format!("attribute `{name}` is reserved")));
        }
    }
    for widget in adornments.iter().filter_map(|a| a.widget.as_deref()) {
        if !is_name(widget, true) {
            return Err(OptionsError(format!("invalid widget name `{widget}`")));
        }
    }
    Ok(())
}

/// `^[a-z][a-z0-9-]*$`, plus `.` when `dotted` (for a widget's `vendor.name`).
fn is_name(s: &str, dotted: bool) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || (dotted && c == '.')
        })
}

/// The text field's typed options: which input variant to render.
#[derive(Deserialize)]
struct TextOptions {
    #[serde(default = "default_input")]
    input: String,
}

fn default_input() -> String {
    "text".to_string()
}

/// The text field's resolved options: everything needed to render and validate,
/// computed (and name-checked) once at boot from the selected input.
struct TextResolved {
    html_type: &'static str,
    attrs: BTreeMap<String, String>,
    adornments: Vec<Adornment>,
    rules: Vec<Rule>,
}

/// The text field's view-model payload. Additive over the prior `{input_type}`,
/// so existing overrides keep working; this shape is the override contract.
#[derive(Default, Serialize, Deserialize)]
struct TextData {
    input_type: String,
    #[serde(default)]
    attrs: BTreeMap<String, String>,
    #[serde(default)]
    adornments: Vec<Adornment>,
}

#[derive(Template)]
#[template(path = "fields/text.html")]
struct TextTmpl<'a> {
    name: &'a str,
    id: &'a str,
    value: &'a str,
    required: bool,
    input_type: &'a str,
    attrs: &'a BTreeMap<String, String>,
    leading: &'a [Adornment],
    trailing: &'a [Adornment],
}

/// A single-line text input. Its `input` option selects an [`InputType`] from
/// the registry (text, email, ...; plugins add more) that drives the HTML type
/// and intrinsic rules.
pub(crate) struct TextField {
    inputs: Arc<InputRegistry>,
}

impl TextField {
    pub(crate) fn new(inputs: Arc<InputRegistry>) -> Self {
        Self { inputs }
    }
}

impl FieldType for TextField {
    fn view_key(&self) -> &'static str {
        "text"
    }
    fn resolve_options(&self, raw: &serde_json::Value) -> Result<ResolvedOptions, OptionsError> {
        let opts: TextOptions = if raw.is_null() {
            TextOptions {
                input: default_input(),
            }
        } else {
            serde_json::from_value(raw.clone()).map_err(|e| OptionsError(e.to_string()))?
        };
        let input = self
            .inputs
            .get(&opts.input)
            .cloned()
            .ok_or_else(|| OptionsError(format!("unknown input type `{}`", opts.input)))?;
        // The input reads its own keys from the same blob, then contributes its
        // control shape; the contributed names are validated before caching.
        let input_opts = input.resolve_options(raw)?;
        let attrs = input.attributes(&input_opts);
        let adornments = input.adornments(&input_opts);
        validate_contributions(&attrs, &adornments)?;
        Ok(ResolvedOptions::new(TextResolved {
            html_type: input.html_type(),
            rules: input.rules(&input_opts),
            attrs,
            adornments,
        }))
    }
    fn intrinsic_rules(&self, opts: &ResolvedOptions) -> Vec<Rule> {
        opts.get::<TextResolved>()
            .map(|r| r.rules.clone())
            .unwrap_or_default()
    }
    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        let data = match cx.opts.get::<TextResolved>() {
            Some(r) => TextData {
                input_type: r.html_type.to_string(),
                attrs: r.attrs.clone(),
                adornments: r.adornments.clone(),
            },
            None => TextData {
                input_type: "text".to_string(),
                ..TextData::default()
            },
        };
        FieldVm {
            view_key: "text".to_string(),
            name: cx.name.to_string(),
            id: cx.id.to_string(),
            label: cx.label.to_string(),
            required: cx.required,
            value: cx.value.clone(),
            data: serde_json::to_value(data).unwrap_or_default(),
        }
    }
    fn render_default(&self, vm: &FieldVm) -> Markup {
        let data: TextData = serde_json::from_value(vm.data.clone()).unwrap_or_default();
        let (leading, trailing): (Vec<Adornment>, Vec<Adornment>) = data
            .adornments
            .into_iter()
            .partition(|a| matches!(a.placement, Placement::Leading));
        Markup::from_template(&TextTmpl {
            name: &vm.name,
            id: &vm.id,
            value: vm.value.as_text(),
            required: vm.required,
            input_type: &data.input_type,
            attrs: &data.attrs,
            leading: &leading,
            trailing: &trailing,
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

/// A `select` field's options: a list of value/label pairs.
#[derive(Debug, Default, Deserialize)]
struct SelectOptions {
    #[serde(default)]
    options: Vec<SelectOption>,
}

#[derive(Debug, Deserialize)]
struct SelectOption {
    value: String,
    /// Display text; defaults to the value when omitted.
    #[serde(default)]
    label: Option<String>,
}

/// One rendered option: label resolved, `selected` computed against the current
/// value. Carried in the view-model so an override presents the same list.
#[derive(Debug, Serialize, Deserialize)]
struct OptionView {
    value: String,
    label: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "fields/select.html")]
struct SelectTmpl<'a> {
    name: &'a str,
    id: &'a str,
    required: bool,
    options: &'a [OptionView],
}

/// A dropdown over a fixed option list.
pub(crate) struct SelectField;

impl FieldType for SelectField {
    fn view_key(&self) -> &'static str {
        "select"
    }
    fn resolve_options(&self, raw: &serde_json::Value) -> Result<ResolvedOptions, OptionsError> {
        let opts: SelectOptions =
            serde_json::from_value(raw.clone()).map_err(|e| OptionsError(e.to_string()))?;
        Ok(ResolvedOptions::new(opts))
    }
    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        let current = cx.value.as_text();
        let views: Vec<OptionView> = cx
            .opts
            .get::<SelectOptions>()
            .map(|o| {
                o.options
                    .iter()
                    .map(|so| OptionView {
                        value: so.value.clone(),
                        label: so.label.clone().unwrap_or_else(|| so.value.clone()),
                        selected: so.value == current,
                    })
                    .collect()
            })
            .unwrap_or_default();
        FieldVm {
            view_key: "select".to_string(),
            name: cx.name.to_string(),
            id: cx.id.to_string(),
            label: cx.label.to_string(),
            required: cx.required,
            value: cx.value.clone(),
            data: serde_json::to_value(&views).unwrap_or_default(),
        }
    }
    fn render_default(&self, vm: &FieldVm) -> Markup {
        let options: Vec<OptionView> = serde_json::from_value(vm.data.clone()).unwrap_or_default();
        Markup::from_template(&SelectTmpl {
            name: &vm.name,
            id: &vm.id,
            required: vm.required,
            options: &options,
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

    fn text_field() -> TextField {
        TextField::new(Arc::new(builtin_input_registry()))
    }

    /// An input that contributes a reserved attribute name, to prove boot rejects it.
    struct BadAttrInput;
    impl InputType for BadAttrInput {
        fn key(&self) -> &'static str {
            "bad"
        }
        fn attributes(&self, _opts: &ResolvedOptions) -> BTreeMap<String, String> {
            BTreeMap::from([("class".to_string(), "x".to_string())])
        }
    }

    #[test]
    fn text_renders_its_value_escaped() {
        let opts = ResolvedOptions::none();
        let value = FieldValue::Text("a<b>&\"c".to_string());
        let markup = render_field(
            &text_field(),
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
        let ft = text_field();
        let vm = ft.view_model(&cx("t", &value, &opts));
        let direct = ft.render_default(&vm);
        let routed = render_field(&ft, &NoOverrides, &scope(), &cx("t", &value, &opts));
        assert_eq!(direct.as_str(), routed.as_str());
    }

    #[test]
    fn text_input_email_sets_the_type_and_contributes_the_email_rule() {
        let field = text_field();
        let opts = field
            .resolve_options(&serde_json::json!({ "input": "email" }))
            .unwrap();
        // The email input variant contributes the Email rule.
        assert!(matches!(
            field.intrinsic_rules(&opts).as_slice(),
            [Rule::Email]
        ));
        let value = FieldValue::Text("a@b.test".to_string());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("email", &value, &opts));
        assert!(markup.as_str().contains(r#"type="email""#));
    }

    #[test]
    fn text_default_input_is_plain_text_with_no_extra_rule() {
        let field = text_field();
        let opts = field.resolve_options(&serde_json::Value::Null).unwrap();
        assert!(field.intrinsic_rules(&opts).is_empty());
        let value = FieldValue::Text("hi".to_string());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("name", &value, &opts));
        assert!(markup.as_str().contains(r#"type="text""#));
    }

    #[test]
    fn text_number_input_sets_the_type_and_numeric_rule() {
        let field = text_field();
        let opts = field
            .resolve_options(&serde_json::json!({ "input": "number" }))
            .unwrap();
        assert!(matches!(
            field.intrinsic_rules(&opts).as_slice(),
            [Rule::Numeric]
        ));
        let value = FieldValue::Text("42".to_string());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("qty", &value, &opts));
        assert!(markup.as_str().contains(r#"type="number""#));
    }

    #[test]
    fn text_tel_input_sets_the_type_with_no_rule() {
        let field = text_field();
        let opts = field
            .resolve_options(&serde_json::json!({ "input": "tel" }))
            .unwrap();
        assert!(field.intrinsic_rules(&opts).is_empty());
        let value = FieldValue::Text(String::new());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("phone", &value, &opts));
        assert!(markup.as_str().contains(r#"type="tel""#));
    }

    #[test]
    fn text_number_input_emits_min_max_step_attributes() {
        let field = text_field();
        let opts = field
            .resolve_options(
                &serde_json::json!({ "input": "number", "min": 0, "max": 10, "step": 2 }),
            )
            .unwrap();
        // Rendering runs vm -> data -> render_default, so attributes surviving into
        // the markup also proves the view-model serialize round-trip.
        let value = FieldValue::Text("5".to_string());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("qty", &value, &opts));
        let html = markup.as_str();
        assert!(html.contains(r#"min="0""#), "{html}");
        assert!(html.contains(r#"max="10""#), "{html}");
        assert!(html.contains(r#"step="2""#), "{html}");
    }

    #[test]
    fn text_input_reserved_attribute_name_aborts_resolve() {
        let mut inputs = builtin_input_registry();
        inputs.insert("bad".to_string(), Arc::new(BadAttrInput));
        let field = TextField::new(Arc::new(inputs));
        let Err(e) = field.resolve_options(&serde_json::json!({ "input": "bad" })) else {
            panic!("expected a reserved-attribute rejection");
        };
        assert!(e.0.contains("class"), "{e}");
    }

    #[test]
    fn attribute_and_widget_name_rules() {
        assert!(is_name("min", false));
        assert!(is_name("data-x", false));
        assert!(!is_name("Min", false));
        assert!(!is_name("min.x", false)); // a dot is not allowed in an attribute name
        assert!(is_name("vendor.copy", true)); // but is in a widget name
        assert!(!is_name("", false));
    }

    #[test]
    fn select_renders_options_with_the_current_value_selected() {
        let raw = serde_json::json!({
            "options": [{"value": "open", "label": "Open"}, {"value": "closed"}]
        });
        let opts = SelectField.resolve_options(&raw).unwrap();
        let value = FieldValue::Text("closed".to_string());
        let markup = render_field(
            &SelectField,
            &NoOverrides,
            &scope(),
            &cx("status", &value, &opts),
        );
        let html = markup.as_str();
        assert!(html.contains(r#"<option value="open">Open</option>"#));
        // The current value is selected; a missing label falls back to the value.
        assert!(html.contains(r#"<option value="closed" selected>closed</option>"#));
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
