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
use laterite_core::validation::{Mode, Rule};
use laterite_core::AttrValue;
use serde::{Deserialize, Serialize};

use crate::html::Markup;
use crate::picker::PickerRegistry;

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
    /// The admin mount path (e.g. `/admin`), so a type that calls an endpoint
    /// (the reference picker) builds its URL without knowing routing conventions.
    pub base: &'a str,
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
/// What a field sees of the submission when it produces its stored value.
///
/// A scalar field wants [`value`](Self::value), the single entry under its own
/// name. A field made of more than one control (a repeater, a checkbox list)
/// reads its own keys with [`nested`](Self::nested): the whole submission is
/// here because a field's shape is the field's business, not the form's.
pub struct SubmittedField<'a> {
    name: &'a str,
    data: &'a HashMap<String, String>,
}

impl<'a> SubmittedField<'a> {
    pub fn new(name: &'a str, data: &'a HashMap<String, String>) -> Self {
        Self { name, data }
    }

    /// This field's submitted name, which a multi-value field uses as the prefix
    /// of its own keys.
    pub fn name(&self) -> &str {
        self.name
    }

    /// The value submitted under this field's own name.
    ///
    /// `None` when the submission omits the field, which is how an unchecked
    /// checkbox arrives.
    pub fn value(&self) -> Option<&str> {
        self.data.get(self.name).map(String::as_str)
    }

    /// Every submitted key beginning `name[`, as its remainder and value.
    ///
    /// `rules[0][path]` on a field named `rules` yields `("0][path]", ..)`; the
    /// field parses the remainder however its own encoding says.
    pub fn nested(&self) -> impl Iterator<Item = (&'a str, &'a str)> + '_ {
        let prefix = format!("{}[", self.name);
        self.data.iter().filter_map(move |(key, value)| {
            key.strip_prefix(&prefix).map(|rest| (rest, value.as_str()))
        })
    }

    /// The whole submission, for a field whose encoding the helpers above do not
    /// cover.
    pub fn all(&self) -> &'a HashMap<String, String> {
        self.data
    }
}

pub trait FieldType: Send + Sync + 'static {
    /// The stable key for override resolution and default-template selection
    /// (bare like `text` for core; dotted `vendor.name` for plugins).
    fn view_key(&self) -> &'static str;

    /// Types the descriptor's raw options blob once at boot. A failure aborts
    /// boot naming the resource + field. Default: no options.
    ///
    /// `types` is the field registry, for a composite field (a repeater) that
    /// holds fields of its own and has to resolve their types and options here,
    /// while the registry is in hand.
    fn resolve_options(
        &self,
        _raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
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

    /// How a submitted value becomes the attribute that gets stored.
    ///
    /// `raw` is `None` when the submission omits the field entirely, which is
    /// how an unchecked checkbox arrives. Returning `None` leaves the attribute
    /// out of the write, so the column keeps whatever it held: that is how a
    /// blank password on an edit means "unchanged" rather than "erase".
    ///
    /// Validation has already run, so a type that needs its input to parse
    /// should guarantee that with an [`intrinsic_rules`](FieldType::intrinsic_rules)
    /// entry and treat the value here as sound. The error is for a genuine
    /// failure (hashing), not a bad value, and surfaces as a failed save.
    ///
    /// Default: the submitted text, unchanged.
    fn to_attr(
        &self,
        field: &SubmittedField<'_>,
        opts: &ResolvedOptions,
        mode: Mode,
    ) -> Result<Option<AttrValue>, String> {
        let _ = (opts, mode);
        Ok(field.value().map(|v| AttrValue::Text(v.to_string())))
    }

    /// How a stored value is presented back in the control on an edit.
    ///
    /// The counterpart of [`to_attr`](FieldType::to_attr): storage and
    /// presentation differ whenever a type stores something other than the text
    /// the browser sent, and a type that never shows its value (a password)
    /// returns the empty string here.
    ///
    /// Default: the stored text, unchanged.
    fn to_control(&self, stored: Option<&str>, opts: &ResolvedOptions) -> String {
        let _ = opts;
        stored.unwrap_or_default().to_string()
    }

    /// Whether the framework wraps this field in standard chrome.
    fn chrome(&self) -> Chrome {
        Chrome::Wrapped
    }

    /// Asset-registry keys this type's control needs on any page that renders it
    /// (a heavy widget's script/style). The page shell collects, dedupes, and
    /// emits them; keys must exist in the asset registry. Default: none, so a
    /// type whose widget ships in core `laterite.js` declares nothing.
    fn assets(&self, _opts: &ResolvedOptions) -> Vec<&'static str> {
        Vec::new()
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

/// A repeater's typed options: the fields one row holds, each resolved to its
/// type and its own options at boot.
pub(crate) struct RepeaterOptions {
    rows: Vec<RepeaterSub>,
    min_items: usize,
    max_items: Option<usize>,
}

/// One sub-field of a repeater row, resolved once.
pub(crate) struct RepeaterSub {
    field: crate::form::FormField,
    field_type: Arc<dyn FieldType>,
    opts: ResolvedOptions,
}

/// The descriptor shape a repeater's `options` blob takes.
#[derive(Deserialize)]
struct RepeaterOptionsRaw {
    fields: Vec<crate::form::FormField>,
    #[serde(default)]
    min_items: usize,
    #[serde(default)]
    max_items: Option<usize>,
}

/// A list of rows, stored as a JSON array of objects.
///
/// Row controls are named `field[index][subfield]`. The indices are read back
/// out of the submitted keys and sorted, so a row removed in the browser leaves
/// no hole and the page renumbers only for tidiness, never for correctness.
///
/// A row's sub-fields are ordinary field types, so a switch inside a row stores
/// a bool and a date inside a row is a date, exactly as at the top level.
pub(crate) struct RepeaterField;

impl RepeaterField {
    /// The placeholder index in the blank row the browser clones.
    const BLANK: &'static str = "__index__";
}

impl FieldType for RepeaterField {
    fn view_key(&self) -> &'static str {
        "repeater"
    }

    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
        let parsed: RepeaterOptionsRaw =
            serde_json::from_value(raw.clone()).map_err(|e| OptionsError(e.to_string()))?;
        let mut rows = Vec::new();
        for field in parsed.fields {
            if field.field_type == "repeater" {
                return Err(OptionsError(
                    "a repeater cannot hold a repeater".to_string(),
                ));
            }
            let field_type = types
                .get(&field.field_type)
                .cloned()
                .ok_or_else(|| OptionsError(format!("unregistered type `{}`", field.field_type)))?;
            let opts = field_type.resolve_options(&field.options, types)?;
            rows.push(RepeaterSub {
                field,
                field_type,
                opts,
            });
        }
        Ok(ResolvedOptions::new(RepeaterOptions {
            rows,
            min_items: parsed.min_items,
            max_items: parsed.max_items,
        }))
    }

    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        let opts = cx.opts.get::<RepeaterOptions>();
        let stored = match cx.value {
            FieldValue::Json(serde_json::Value::Array(rows)) => rows.clone(),
            _ => Vec::new(),
        };
        let data = RepeaterData {
            rows: opts
                .map(|o| {
                    stored
                        .iter()
                        .enumerate()
                        .map(|(index, row)| render_row(o, cx, &index.to_string(), Some(row)))
                        .collect()
                })
                .unwrap_or_default(),
            blank: opts
                .map(|o| render_row(o, cx, Self::BLANK, None))
                .unwrap_or_default(),
        };

        FieldVm {
            view_key: self.view_key().to_string(),
            name: cx.name.to_string(),
            id: cx.id.to_string(),
            label: cx.label.to_string(),
            required: cx.required,
            value: cx.value.clone(),
            data: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn render_default(&self, vm: &FieldVm) -> Markup {
        let data: RepeaterData = serde_json::from_value(vm.data.clone()).unwrap_or_default();
        Markup::from_template(&RepeaterTmpl { data }).unwrap_or_default()
    }

    fn to_attr(
        &self,
        field: &SubmittedField<'_>,
        opts: &ResolvedOptions,
        mode: Mode,
    ) -> Result<Option<AttrValue>, String> {
        let Some(options) = opts.get::<RepeaterOptions>() else {
            return Ok(Some(AttrValue::Json(serde_json::Value::Array(Vec::new()))));
        };

        // Indices come from the keys, sorted, so a gap left by a removed row
        // costs nothing and the client never has to renumber for correctness.
        let mut indices: Vec<usize> = field
            .nested()
            .filter_map(|(rest, _)| rest.split_once(']').and_then(|(i, _)| i.parse().ok()))
            .collect();
        indices.sort_unstable();
        indices.dedup();

        let mut rows = Vec::new();
        for index in indices {
            let mut object = serde_json::Map::new();
            for sub in &options.rows {
                let key = format!("{}[{index}][{}]", field.name(), sub.field.name);
                let scoped = SubmittedField::new(&key, field.all());
                if let Some(value) = sub.field_type.to_attr(&scoped, &sub.opts, mode)? {
                    object.insert(sub.field.name.clone(), attr_to_json(value));
                }
            }
            let row = serde_json::Value::Object(object);
            // A row the operator added and left empty is not a row.
            if !is_blank_row(&row) {
                rows.push(row);
            }
        }

        if rows.len() < options.min_items {
            return Err(format!("at least {} required", options.min_items));
        }
        if let Some(max) = options.max_items {
            if rows.len() > max {
                return Err(format!("at most {max} allowed"));
            }
        }
        Ok(Some(AttrValue::Json(serde_json::Value::Array(rows))))
    }

    fn to_control(&self, stored: Option<&str>, _opts: &ResolvedOptions) -> String {
        stored.unwrap_or("[]").to_string()
    }
}

/// One row's sub-fields, rendered. `row` is `None` for the blank row.
fn render_row(
    options: &RepeaterOptions,
    cx: &FieldCx<'_>,
    index: &str,
    row: Option<&serde_json::Value>,
) -> Vec<RepeaterCell> {
    options
        .rows
        .iter()
        .map(|sub| {
            let name = format!("{}[{index}][{}]", cx.name, sub.field.name);
            let stored = row.and_then(|r| r.get(&sub.field.name));
            let value = match stored {
                Some(serde_json::Value::String(s)) => FieldValue::Text(s.clone()),
                Some(serde_json::Value::Null) | None => FieldValue::Text(String::new()),
                Some(other) => FieldValue::Text(
                    sub.field_type
                        .to_control(Some(&other.to_string()), &sub.opts),
                ),
            };
            let sub_cx = FieldCx {
                name: &name,
                id: &name,
                label: cx.label,
                value: &value,
                required: false,
                opts: &sub.opts,
                base: cx.base,
            };
            let vm = sub.field_type.view_model(&sub_cx);
            RepeaterCell {
                // Sub-labels render from their source string: a field type has
                // no request translator, and nested labels are the first strings
                // to need one. Recorded against 7.1.
                label: sub.field.label.source().to_string(),
                control: sub.field_type.render_default(&vm).into_string(),
            }
        })
        .collect()
}

/// A stored attribute as the JSON a repeater row holds.
fn attr_to_json(value: AttrValue) -> serde_json::Value {
    match value {
        AttrValue::Null => serde_json::Value::Null,
        AttrValue::Bool(b) => serde_json::Value::Bool(b),
        AttrValue::Int(i) => serde_json::Value::from(i),
        AttrValue::Float(f) => serde_json::Value::from(f),
        AttrValue::Json(j) => j,
        other => serde_json::Value::String(other.to_text().unwrap_or_default()),
    }
}

/// Whether every value in a row is empty or false.
fn is_blank_row(row: &serde_json::Value) -> bool {
    row.as_object().is_none_or(|object| {
        object.values().all(|v| match v {
            serde_json::Value::String(s) => s.trim().is_empty(),
            serde_json::Value::Bool(b) => !b,
            serde_json::Value::Array(items) => items.is_empty(),
            serde_json::Value::Null => true,
            _ => false,
        })
    })
}

/// One rendered cell of a repeater row.
#[derive(Serialize, Deserialize)]
struct RepeaterCell {
    label: String,
    control: String,
}

/// A repeater's presentation payload, typed on the way out and back so the
/// template reads fields rather than poking at JSON.
#[derive(Serialize, Deserialize, Default)]
struct RepeaterData {
    rows: Vec<Vec<RepeaterCell>>,
    blank: Vec<RepeaterCell>,
}

#[derive(Template)]
#[template(path = "fields/repeater.html")]
struct RepeaterTmpl {
    data: RepeaterData,
}

/// The framework's built-in field types. The text field is constructed with the
/// built-in input-type registry it delegates to.
pub(crate) fn builtin_field_types() -> Vec<Arc<dyn FieldType>> {
    let inputs = Arc::new(builtin_input_registry());
    vec![
        Arc::new(TextField::new(inputs)),
        Arc::new(TextareaField),
        Arc::new(RepeaterField),
        Arc::new(SelectField),
        Arc::new(SwitchField),
        Arc::new(DateField),
        Arc::new(PasswordField),
        Arc::new(RadioField),
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
    fn resolve_options(
        &self,
        _raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
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
    /// Asset-registry keys this input needs (a heavy widget's script/style). The
    /// copy button needs none, since its island ships in core `laterite.js`.
    fn assets(&self, _opts: &ResolvedOptions) -> Vec<&'static str> {
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
    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
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

/// A url input's option: whether to show the copy-to-clipboard button.
#[derive(Deserialize)]
struct UrlOptions {
    #[serde(default = "default_true")]
    copy: bool,
}

impl Default for UrlOptions {
    fn default() -> Self {
        Self { copy: true }
    }
}

fn default_true() -> bool {
    true
}

struct UrlInput;
impl InputType for UrlInput {
    fn key(&self) -> &'static str {
        "url"
    }
    fn html_type(&self) -> &'static str {
        "url"
    }
    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
        let opts: UrlOptions = if raw.is_null() {
            UrlOptions::default()
        } else {
            serde_json::from_value(raw.clone()).map_err(|e| OptionsError(e.to_string()))?
        };
        Ok(ResolvedOptions::new(opts))
    }
    fn rules(&self, _opts: &ResolvedOptions) -> Vec<Rule> {
        vec![Rule::Url]
    }
    fn adornments(&self, opts: &ResolvedOptions) -> Vec<Adornment> {
        if opts.get::<UrlOptions>().is_none_or(|o| o.copy) {
            vec![Adornment {
                placement: Placement::Trailing,
                button: true,
                label: "Copy".to_string(),
                widget: Some("copy".to_string()),
            }]
        } else {
            Vec::new()
        }
    }
}

pub(crate) fn builtin_input_types() -> Vec<Arc<dyn InputType>> {
    vec![
        Arc::new(TextInput),
        Arc::new(EmailInput),
        Arc::new(TelInput),
        Arc::new(NumberInput),
        Arc::new(UrlInput),
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
pub(crate) fn is_name(s: &str, dotted: bool) -> bool {
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
    assets: Vec<&'static str>,
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
    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
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
        let input_opts = input.resolve_options(raw, _types)?;
        let attrs = input.attributes(&input_opts);
        let adornments = input.adornments(&input_opts);
        validate_contributions(&attrs, &adornments)?;
        Ok(ResolvedOptions::new(TextResolved {
            html_type: input.html_type(),
            rules: input.rules(&input_opts),
            assets: input.assets(&input_opts),
            attrs,
            adornments,
        }))
    }
    fn intrinsic_rules(&self, opts: &ResolvedOptions) -> Vec<Rule> {
        opts.get::<TextResolved>()
            .map(|r| r.rules.clone())
            .unwrap_or_default()
    }
    fn assets(&self, opts: &ResolvedOptions) -> Vec<&'static str> {
        opts.get::<TextResolved>()
            .map(|r| r.assets.clone())
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
    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
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

#[derive(Template)]
#[template(path = "fields/switch.html")]
struct SwitchTmpl<'a> {
    name: &'a str,
    id: &'a str,
    on: bool,
}

/// A boolean toggle over a bool column.
///
/// The only field whose submission can be absent rather than empty: a browser
/// sends nothing for an unchecked box, which is why absence has to mean `false`
/// here rather than "leave it alone".
pub(crate) struct SwitchField;

/// Whether a stored value reads as true. Accepts what each backend gives back
/// for a boolean column, since `sqlx::Any` reads them all as text.
fn stored_is_on(stored: Option<&str>) -> bool {
    matches!(
        stored.map(str::trim).unwrap_or_default(),
        "1" | "true" | "TRUE" | "t" | "yes" | "on"
    )
}

impl FieldType for SwitchField {
    fn view_key(&self) -> &'static str {
        "switch"
    }

    fn to_attr(
        &self,
        field: &SubmittedField<'_>,
        _opts: &ResolvedOptions,
        _mode: Mode,
    ) -> Result<Option<AttrValue>, String> {
        // Absent means unchecked, so this always writes a value: leaving the
        // attribute out would keep the old one and the box would never clear.
        Ok(Some(AttrValue::Bool(stored_is_on(field.value()))))
    }

    fn to_control(&self, stored: Option<&str>, _opts: &ResolvedOptions) -> String {
        if stored_is_on(stored) {
            "1".to_string()
        } else {
            String::new()
        }
    }

    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        scalar_vm("switch", cx)
    }

    fn render_default(&self, vm: &FieldVm) -> Markup {
        Markup::from_template(&SwitchTmpl {
            name: &vm.name,
            id: &vm.id,
            on: stored_is_on(Some(vm.value.as_text())),
        })
        .unwrap_or_default()
    }
}

#[derive(Template)]
#[template(path = "fields/date.html")]
struct DateTmpl<'a> {
    name: &'a str,
    id: &'a str,
    value: &'a str,
    required: bool,
}

/// A calendar date.
///
/// Stores `YYYY-MM-DD`, the format a date control sends. Presenting is where the
/// work is: the column may hold a full timestamp, which a date input rejects, so
/// a stored value is trimmed back to its date.
pub(crate) struct DateField;

impl FieldType for DateField {
    fn view_key(&self) -> &'static str {
        "date"
    }

    fn intrinsic_rules(&self, _opts: &ResolvedOptions) -> Vec<Rule> {
        vec![Rule::Date]
    }

    fn to_attr(
        &self,
        field: &SubmittedField<'_>,
        _opts: &ResolvedOptions,
        _mode: Mode,
    ) -> Result<Option<AttrValue>, String> {
        Ok(field.value().map(|v| match v.trim() {
            "" => AttrValue::Null,
            date => AttrValue::Text(date.to_string()),
        }))
    }

    fn to_control(&self, stored: Option<&str>, _opts: &ResolvedOptions) -> String {
        let stored = stored.unwrap_or_default().trim();
        // A timestamp column hands back the whole instant; the control wants the
        // date, and everything the framework stores starts with one.
        stored
            .split(['T', ' '])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        scalar_vm("date", cx)
    }

    fn render_default(&self, vm: &FieldVm) -> Markup {
        Markup::from_template(&DateTmpl {
            name: &vm.name,
            id: &vm.id,
            value: vm.value.as_text(),
            required: vm.required,
        })
        .unwrap_or_default()
    }
}

#[derive(Template)]
#[template(path = "fields/password.html")]
struct PasswordTmpl<'a> {
    name: &'a str,
    id: &'a str,
    required: bool,
}

/// A password: hashed on the way in, never shown on the way out.
///
/// Both halves matter. The control renders with no value, so a stored hash
/// cannot leak into the page, and a blank submission omits the attribute, so
/// editing a record without touching its password leaves that password alone.
/// Require it on create (`required_on(Mode::Create)`) if a blank one is not
/// acceptable there.
pub(crate) struct PasswordField;

impl FieldType for PasswordField {
    fn view_key(&self) -> &'static str {
        "password"
    }

    fn to_attr(
        &self,
        field: &SubmittedField<'_>,
        _opts: &ResolvedOptions,
        _mode: Mode,
    ) -> Result<Option<AttrValue>, String> {
        match field.value().map(str::trim).unwrap_or_default() {
            // Absent or blank: leave the stored password as it is.
            "" => Ok(None),
            plain => laterite_auth::password::hash_password(plain)
                .map(|hash| Some(AttrValue::Text(hash)))
                .map_err(|e| e.to_string()),
        }
    }

    /// Never echoes: a hash must not reach the page, and a browser must not be
    /// invited to refill it.
    fn to_control(&self, _stored: Option<&str>, _opts: &ResolvedOptions) -> String {
        String::new()
    }

    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        scalar_vm("password", cx)
    }

    fn render_default(&self, vm: &FieldVm) -> Markup {
        Markup::from_template(&PasswordTmpl {
            name: &vm.name,
            id: &vm.id,
            required: vm.required,
        })
        .unwrap_or_default()
    }
}

#[derive(Template)]
#[template(path = "fields/radio.html")]
struct RadioTmpl<'a> {
    name: &'a str,
    options: &'a [OptionView],
    required: bool,
}

/// One choice from a fixed list, shown as radios rather than a dropdown.
///
/// The same options as `select`, so a descriptor swaps between them by changing
/// the type alone. Suits a short list where seeing every choice matters.
pub(crate) struct RadioField;

impl FieldType for RadioField {
    fn view_key(&self) -> &'static str {
        "radio"
    }

    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
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
            view_key: "radio".to_string(),
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
        Markup::from_template(&RadioTmpl {
            name: &vm.name,
            options: &options,
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

/// The reference-picker field's option: which registered source to pick from.
#[derive(Deserialize)]
struct RefOptions {
    source: String,
}

/// The reference-picker's resolved options: the source name, validated at boot.
struct RefResolved {
    source: String,
}

/// The reference-picker's view-model payload: the source and the endpoint URLs
/// its widget calls, plus the stored id (which the hidden input submits).
#[derive(Default, Serialize, Deserialize)]
struct RefData {
    source: String,
    search_url: String,
    resolve_url: String,
    value: String,
    placeholder: String,
}

#[derive(Template)]
#[template(path = "fields/ref_picker.html")]
struct RefPickerTmpl<'a> {
    name: &'a str,
    id: &'a str,
    value: &'a str,
    search_url: &'a str,
    resolve_url: &'a str,
    placeholder: &'a str,
}

/// A picker over a reference to another record: stores that record's id, shows
/// its label, and offers typeahead candidates from a registered [`PickerSource`]
/// (selected by the `source` option). The label and candidates are fetched by
/// its widget from the source's endpoints; rendering stays pure.
pub(crate) struct RefPickerField {
    pickers: Arc<PickerRegistry>,
}

impl RefPickerField {
    pub(crate) fn new(pickers: Arc<PickerRegistry>) -> Self {
        Self { pickers }
    }
}

impl FieldType for RefPickerField {
    fn view_key(&self) -> &'static str {
        "reference"
    }
    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
        let opts: RefOptions =
            serde_json::from_value(raw.clone()).map_err(|e| OptionsError(e.to_string()))?;
        if !self.pickers.contains_key(&opts.source) {
            return Err(OptionsError(format!(
                "unknown picker source `{}`",
                opts.source
            )));
        }
        Ok(ResolvedOptions::new(RefResolved {
            source: opts.source,
        }))
    }
    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        let source = cx
            .opts
            .get::<RefResolved>()
            .map(|r| r.source.as_str())
            .unwrap_or_default();
        let data = RefData {
            search_url: format!("{}/pickers/{}/search", cx.base, source),
            resolve_url: format!("{}/pickers/{}/resolve", cx.base, source),
            source: source.to_string(),
            value: cx.value.as_text().to_string(),
            placeholder: "Search…".to_string(),
        };
        FieldVm {
            view_key: "reference".to_string(),
            name: cx.name.to_string(),
            id: cx.id.to_string(),
            label: cx.label.to_string(),
            required: cx.required,
            value: cx.value.clone(),
            data: serde_json::to_value(data).unwrap_or_default(),
        }
    }
    fn render_default(&self, vm: &FieldVm) -> Markup {
        let data: RefData = serde_json::from_value(vm.data.clone()).unwrap_or_default();
        Markup::from_template(&RefPickerTmpl {
            name: &vm.name,
            id: &vm.id,
            value: vm.value.as_text(),
            search_url: &data.search_url,
            resolve_url: &data.resolve_url,
            placeholder: &data.placeholder,
        })
        .unwrap_or_default()
    }
    fn assets(&self, _opts: &ResolvedOptions) -> Vec<&'static str> {
        vec!["fields/ref-picker.js", "fields/ref-picker.css"]
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
            base: "/admin",
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
            .resolve_options(
                &serde_json::json!({ "input": "email" }),
                &builtin_registry(),
            )
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
        let opts = field
            .resolve_options(&serde_json::Value::Null, &builtin_registry())
            .unwrap();
        assert!(field.intrinsic_rules(&opts).is_empty());
        let value = FieldValue::Text("hi".to_string());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("name", &value, &opts));
        assert!(markup.as_str().contains(r#"type="text""#));
    }

    #[test]
    fn text_number_input_sets_the_type_and_numeric_rule() {
        let field = text_field();
        let opts = field
            .resolve_options(
                &serde_json::json!({ "input": "number" }),
                &builtin_registry(),
            )
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
            .resolve_options(&serde_json::json!({ "input": "tel" }), &builtin_registry())
            .unwrap();
        assert!(field.intrinsic_rules(&opts).is_empty());
        let value = FieldValue::Text(String::new());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("phone", &value, &opts));
        assert!(markup.as_str().contains(r#"type="tel""#));
    }

    #[test]
    fn text_url_input_sets_the_type_url_rule_and_copy_button() {
        let field = text_field();
        let opts = field
            .resolve_options(&serde_json::json!({ "input": "url" }), &builtin_registry())
            .unwrap();
        assert!(matches!(
            field.intrinsic_rules(&opts).as_slice(),
            [Rule::Url]
        ));
        let value = FieldValue::Text("https://example.com".to_string());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("site", &value, &opts));
        let html = markup.as_str();
        assert!(html.contains(r#"type="url""#), "{html}");
        assert!(html.contains("lat-input-group"), "{html}");
        assert!(html.contains(r#"data-lat-widget="copy""#), "{html}");
    }

    #[test]
    fn text_url_input_copy_false_omits_the_button() {
        let field = text_field();
        let opts = field
            .resolve_options(
                &serde_json::json!({ "input": "url", "copy": false }),
                &builtin_registry(),
            )
            .unwrap();
        let value = FieldValue::Text("https://example.com".to_string());
        let markup = render_field(&field, &NoOverrides, &scope(), &cx("site", &value, &opts));
        let html = markup.as_str();
        assert!(html.contains(r#"type="url""#), "{html}");
        // No adornment, so a plain input with no group and no widget hook.
        assert!(!html.contains("lat-input-group"), "{html}");
        assert!(!html.contains("data-lat-widget"), "{html}");
    }

    #[test]
    fn text_number_input_emits_min_max_step_attributes() {
        let field = text_field();
        let opts = field
            .resolve_options(
                &serde_json::json!({ "input": "number", "min": 0, "max": 10, "step": 2 }),
                &builtin_registry(),
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
        let Err(e) =
            field.resolve_options(&serde_json::json!({ "input": "bad" }), &builtin_registry())
        else {
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
        let opts = SelectField
            .resolve_options(&raw, &builtin_registry())
            .unwrap();
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

    #[test]
    fn radio_renders_the_same_options_with_the_current_one_checked() {
        let raw = serde_json::json!({
            "options": [{"value": "open", "label": "Open"}, {"value": "closed"}]
        });
        let opts = RadioField
            .resolve_options(&raw, &builtin_registry())
            .unwrap();
        let value = FieldValue::Text("closed".to_string());
        let markup = render_field(
            &RadioField,
            &NoOverrides,
            &scope(),
            &cx("status", &value, &opts),
        );
        let html = markup.as_str();
        assert!(html.contains(r#"value="open""#));
        assert!(
            html.contains(r#"value="closed" checked"#),
            "current is checked"
        );
        // Every radio shares the field's name, or the browser treats them as
        // separate controls and lets more than one be chosen.
        assert_eq!(html.matches(r#"name="status""#).count(), 2);
        // A missing label falls back to the value, as select does.
        assert!(html.contains("closed</label>"));
    }

    fn scope<'a>() -> OverrideScope<'a> {
        OverrideScope {
            surface: Surface::Field,
            view_key: "text",
            resource: None,
            field: None,
        }
    }

    fn ref_field() -> RefPickerField {
        let mut pickers = crate::picker::PickerRegistry::new();
        let source = Arc::new(crate::picker::TableSource::new("places", "id", "name"));
        pickers.insert(
            "acme.place".to_string(),
            crate::picker::PickerSourceReg::new("acme.place", source),
        );
        RefPickerField::new(Arc::new(pickers))
    }

    #[test]
    fn reference_rejects_an_unknown_source() {
        let field = ref_field();
        let Err(e) = field.resolve_options(
            &serde_json::json!({ "source": "no.such" }),
            &builtin_registry(),
        ) else {
            panic!("expected an unknown-source rejection");
        };
        assert!(e.0.contains("no.such"), "{e}");
    }

    #[test]
    fn reference_renders_a_preserved_hidden_id_and_the_combobox() {
        let field = ref_field();
        let opts = field
            .resolve_options(
                &serde_json::json!({ "source": "acme.place" }),
                &builtin_registry(),
            )
            .unwrap();
        let value = FieldValue::Text("42".to_string());
        let markup = render_field(
            &field,
            &NoOverrides,
            &scope(),
            &cx("place_id", &value, &opts),
        );
        let html = markup.as_str();
        // The stored id rides in the hidden input the form submits, so an untouched
        // form (or one with JS off) preserves the reference.
        assert!(html.contains(r#"type="hidden""#), "{html}");
        assert!(html.contains(r#"name="place_id""#), "{html}");
        assert!(html.contains(r#"value="42""#), "{html}");
        // The widget hook and its endpoint URLs (built from the mount path).
        assert!(html.contains(r#"data-lat-widget="ref-picker""#), "{html}");
        assert!(html.contains("/admin/pickers/acme.place/search"), "{html}");
        assert!(html.contains("/admin/pickers/acme.place/resolve"), "{html}");
    }

    #[test]
    fn reference_declares_its_widget_assets() {
        let field = ref_field();
        let opts = field
            .resolve_options(
                &serde_json::json!({ "source": "acme.place" }),
                &builtin_registry(),
            )
            .unwrap();
        assert_eq!(
            field.assets(&opts),
            vec!["fields/ref-picker.js", "fields/ref-picker.css"]
        );
    }
}

#[cfg(test)]
mod repeater_tests {
    use super::*;

    fn options() -> ResolvedOptions {
        RepeaterField
            .resolve_options(
                &serde_json::json!({
                    "fields": [
                        { "name": "path", "label": "Path", "type": "text" },
                        { "name": "allow", "label": "Allow", "type": "switch" },
                    ]
                }),
                &builtin_registry(),
            )
            .unwrap()
    }

    fn submitted(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn stored(data: &HashMap<String, String>, opts: &ResolvedOptions) -> serde_json::Value {
        let field = SubmittedField::new("rules", data);
        match RepeaterField.to_attr(&field, opts, Mode::Create).unwrap() {
            Some(AttrValue::Json(value)) => value,
            other => panic!("expected a JSON array, got {other:?}"),
        }
    }

    /// A row's sub-fields go through their own field types, so a switch inside a
    /// row stores a bool rather than the string "on". This is the whole reason
    /// the repeater belongs in the field system.
    #[test]
    fn a_row_is_typed_by_its_sub_field_types() {
        let rows = stored(
            &submitted(&[
                ("rules[0][path]", "/private"),
                ("rules[1][path]", "/public"),
                ("rules[1][allow]", "on"),
            ]),
            &options(),
        );

        assert_eq!(rows[0]["path"], "/private");
        assert_eq!(rows[0]["allow"], serde_json::json!(false));
        assert_eq!(rows[1]["allow"], serde_json::json!(true));
    }

    /// Removing a row in the browser can leave a gap. Indices are read from the
    /// keys and sorted, so a gap costs nothing.
    #[test]
    fn rows_are_ordered_by_index_not_submission_order() {
        let rows = stored(
            &submitted(&[("rules[7][path]", "/last"), ("rules[2][path]", "/first")]),
            &options(),
        );
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(rows[0]["path"], "/first");
        assert_eq!(rows[1]["path"], "/last");
    }

    #[test]
    fn a_row_left_blank_is_dropped() {
        let rows = stored(
            &submitted(&[("rules[0][path]", "/kept"), ("rules[1][path]", "  ")]),
            &options(),
        );
        assert_eq!(rows.as_array().unwrap().len(), 1);
    }

    #[test]
    fn nothing_submitted_stores_an_empty_list() {
        assert_eq!(stored(&submitted(&[]), &options()), serde_json::json!([]));
    }

    #[test]
    fn a_repeater_refuses_to_hold_a_repeater() {
        let err = RepeaterField
            .resolve_options(
                &serde_json::json!({
                    "fields": [{ "name": "inner", "label": "Inner", "type": "repeater" }]
                }),
                &builtin_registry(),
            )
            .err()
            .expect("a nested repeater is refused");
        assert!(err.0.contains("cannot hold a repeater"), "{}", err.0);
    }

    #[test]
    fn an_unregistered_sub_field_type_is_refused_at_boot() {
        let err = RepeaterField
            .resolve_options(
                &serde_json::json!({
                    "fields": [{ "name": "x", "label": "X", "type": "acme.nope" }]
                }),
                &builtin_registry(),
            )
            .err()
            .expect("an unregistered type is refused");
        assert!(err.0.contains("acme.nope"), "{}", err.0);
    }

    #[test]
    fn row_counts_are_bounded_when_asked() {
        let opts = RepeaterField
            .resolve_options(
                &serde_json::json!({
                    "fields": [{ "name": "path", "label": "Path", "type": "text" }],
                    "min_items": 1,
                    "max_items": 2
                }),
                &builtin_registry(),
            )
            .unwrap();

        let empty = HashMap::new();
        let too_few = SubmittedField::new("rules", &empty);
        assert!(RepeaterField
            .to_attr(&too_few, &opts, Mode::Create)
            .is_err());

        let three = submitted(&[
            ("rules[0][path]", "/a"),
            ("rules[1][path]", "/b"),
            ("rules[2][path]", "/c"),
        ]);
        let field = SubmittedField::new("rules", &three);
        assert!(RepeaterField.to_attr(&field, &opts, Mode::Create).is_err());
    }

    #[test]
    fn it_renders_a_row_per_stored_entry_plus_a_blank_to_clone() {
        let value = FieldValue::Json(serde_json::json!([
            { "path": "/private", "allow": false },
        ]));
        let cx = FieldCx {
            name: "rules",
            id: "rules",
            label: "Rules",
            value: &value,
            required: false,
            opts: &options(),
            base: "/admin",
        };
        let html = RepeaterField
            .render_default(&RepeaterField.view_model(&cx))
            .into_string();

        assert!(html.contains(r#"name="rules[0][path]""#));
        assert!(html.contains(r#"value="/private""#));
        // The blank row is inert inside a <template> until the browser clones it.
        assert!(html.contains("lat-repeater__blank"));
        assert!(html.contains(r#"name="rules[__index__][path]""#));
    }
}

#[cfg(test)]
mod save_contract_tests {
    use super::*;

    /// One scalar submission, the way the form builds it for a field.
    fn one(raw: Option<&str>) -> HashMap<String, String> {
        raw.into_iter()
            .map(|v| ("f".to_string(), v.to_string()))
            .collect()
    }

    fn none() -> ResolvedOptions {
        ResolvedOptions::none()
    }

    #[test]
    fn a_switch_stores_a_bool_and_absence_means_off() {
        let f = SwitchField;
        // A browser sends nothing for an unchecked box, so absence must write
        // false rather than leave the column as it was.
        assert_eq!(
            f.to_attr(&SubmittedField::new("f", &one(None)), &none(), Mode::Update)
                .unwrap(),
            Some(AttrValue::Bool(false))
        );
        assert_eq!(
            f.to_attr(
                &SubmittedField::new("f", &one(Some("on"))),
                &none(),
                Mode::Create
            )
            .unwrap(),
            Some(AttrValue::Bool(true))
        );
    }

    #[test]
    fn a_switch_reads_back_whatever_the_backend_gave() {
        let f = SwitchField;
        // sqlx::Any hands every backend's boolean back as text, and they differ.
        for on in ["1", "true", "t", "on"] {
            assert_eq!(f.to_control(Some(on), &none()), "1", "{on} reads as on");
        }
        for off in ["0", "false", "f", ""] {
            assert!(f.to_control(Some(off), &none()).is_empty(), "{off} is off");
        }
        assert!(f.to_control(None, &none()).is_empty());
    }

    #[test]
    fn a_date_shows_only_the_date_part_of_a_stored_instant() {
        let f = DateField;
        // A timestamp column hands back the whole instant; a date input rejects it.
        assert_eq!(
            f.to_control(Some("2026-09-09T12:30:45.000000Z"), &none()),
            "2026-09-09"
        );
        assert_eq!(
            f.to_control(Some("2026-09-09 12:30:45"), &none()),
            "2026-09-09"
        );
        assert_eq!(f.to_control(Some("2026-09-09"), &none()), "2026-09-09");
        assert_eq!(f.to_control(None, &none()), "");
    }

    #[test]
    fn a_cleared_date_stores_null_not_an_empty_string() {
        let f = DateField;
        assert_eq!(
            f.to_attr(
                &SubmittedField::new("f", &one(Some(""))),
                &none(),
                Mode::Update
            )
            .unwrap(),
            Some(AttrValue::Null)
        );
        assert_eq!(
            f.to_attr(
                &SubmittedField::new("f", &one(Some("2026-09-09"))),
                &none(),
                Mode::Create
            )
            .unwrap(),
            Some(AttrValue::Text("2026-09-09".into()))
        );
        assert!(matches!(f.intrinsic_rules(&none())[..], [Rule::Date]));
    }

    #[test]
    fn a_password_hashes_and_never_echoes() {
        let f = PasswordField;
        let stored = f
            .to_attr(
                &SubmittedField::new("f", &one(Some("hunter2hunter2"))),
                &none(),
                Mode::Create,
            )
            .unwrap()
            .unwrap();
        let hash = stored.as_str().unwrap();
        assert!(
            hash.starts_with("$argon2"),
            "stored as a hash, not plaintext"
        );
        assert_ne!(hash, "hunter2hunter2");
        // The hash must never reach the page, whatever is stored.
        assert_eq!(f.to_control(Some(hash), &none()), "");
    }

    #[test]
    fn a_blank_password_on_an_edit_leaves_the_stored_one_alone() {
        let f = PasswordField;
        // Omitted, so the write does not touch the column.
        assert_eq!(
            f.to_attr(
                &SubmittedField::new("f", &one(Some(""))),
                &none(),
                Mode::Update
            )
            .unwrap(),
            None
        );
        assert_eq!(
            f.to_attr(
                &SubmittedField::new("f", &one(Some("   "))),
                &none(),
                Mode::Update
            )
            .unwrap(),
            None
        );
        assert_eq!(
            f.to_attr(&SubmittedField::new("f", &one(None)), &none(), Mode::Update)
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_plain_field_stores_and_shows_the_text_unchanged() {
        let f = TextareaField;
        assert_eq!(
            f.to_attr(
                &SubmittedField::new("f", &one(Some("hello"))),
                &none(),
                Mode::Create
            )
            .unwrap(),
            Some(AttrValue::Text("hello".into()))
        );
        // Absent stays absent, so the write leaves that column alone.
        assert_eq!(
            f.to_attr(&SubmittedField::new("f", &one(None)), &none(), Mode::Update)
                .unwrap(),
            None
        );
        assert_eq!(f.to_control(Some("hello"), &none()), "hello");
    }
}
