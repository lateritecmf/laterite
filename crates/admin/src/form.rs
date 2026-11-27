//! Descriptor-driven create and edit forms.
//!
//! A [`FormConfig`] describes a table and its editable fields. Generic handlers
//! render an empty form (new), a populated form (edit), and persist through a
//! [`crate::persist::Persister`]: the descriptor-driven default (a parameterized
//! insert/update over the form's columns), or a named handler for a custom write.
//! A submission is checked by the framework validation engine
//! ([`laterite_core::validation`]); a failure re-renders the form with per-field
//! messages instead of writing.
//!
//! Field types resolve through the registry ([`crate::field`]); `text`,
//! `textarea`, `select`, and `reference` ship built-in. Fields needing a typed
//! stored value (a switch over a bool column, password hashing) arrive with the
//! typed-save contract.
//!
//! The primary key is a `bigint` auto-increment column the database assigns, so
//! create inserts only the descriptor's fields and never sets the id. An entity
//! with other required columns that lack defaults (for example audit timestamps)
//! is beyond this slice; those are filled by a later timestamp-aware widget.

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use laterite_core::query::{bind_values, build as to_sql, text_cast};
use laterite_core::validation::{validate, FieldRules, Mode, Rule};
use laterite_core::{
    t, Actor, AnyRowExt, Db, ErrorBag, ModelListener, ModelListenerReg, Op, Record, Text,
};
use sea_query::{Alias, Expr, Query};
use serde::{Deserialize, Serialize};

use crate::field::{render_field, FieldCx, FieldValue, OverrideScope, ResolvedOptions, Surface};
use crate::persist::{self, DefaultPersister, Persister, PersisterRegistry, SaveError};
use crate::sql::valid_ident;
use crate::{not_found, render, render_error, AdminState};

/// One editable field: the column, its label, its field-type key (resolved
/// through the field-type registry), typed options for that type, the validation
/// rules it carries, and whether it holds translatable content. A serde
/// descriptor, so it is authorable as data (later YAML) as well as by builder.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FormField {
    /// The column the field reads and writes, and the name it submits under.
    pub name: String,
    /// The field's label, localized at render. Serde stays a plain string.
    pub label: Text,
    /// The field-type registry key (`text`, `textarea`, or a plugin's
    /// `vendor.name`), resolved to behaviour at render time (see [`crate::field`]).
    pub field_type: String,
    /// The keys that belong to the field's type, as an object: in a file they
    /// are written on the field itself, beside its own. Null when there are none.
    pub options: serde_json::Value,
    /// Validation rules run on submit (see [`laterite_core::validation`]).
    pub rules: Vec<Rule>,
    /// Marks a field whose value is translatable content. A reserved seam: the
    /// framework stores the value verbatim; a content-translation plugin reads
    /// the flag to manage per-locale values.
    pub translatable: bool,
    /// Help text shown beneath the control, localized at render.
    pub help: Option<Text>,
    /// Text shown above the control, under the label, localized at render.
    pub help_above: Option<Text>,
    /// Classes added to the field's wrapper.
    pub css_class: Option<String>,
    /// Attributes added to the field's wrapper, by name.
    pub container_attributes: std::collections::BTreeMap<String, String>,
    /// What Enter does in this field: `submit` (the form's rule), `off`, or
    /// `next` (moves to the next field).
    pub enter: FieldEnter,
    /// The share of the row the field takes, on a twelve-column grid.
    pub span: Span,
    /// Starts a new row, whatever room the current one has left.
    pub break_row: bool,
    /// Follows another field as it is typed, until this one is edited.
    pub preset: Option<Preset>,
    /// Submitted but never shown: a plain hidden input carrying its value. The
    /// foreign key of a related record's form is one.
    pub hidden: bool,
    /// Shown and submitted with no column behind it: the loader does not
    /// select it and the built-in writer leaves it out, so a persister reads
    /// it from the record (a password set through a form, say).
    pub transient: bool,
    /// The one-to-one record this field is stored on, by its name under the
    /// form's `related`; `None` stores it on the form's own record.
    pub related: Option<String>,
    /// The pages the field is on: empty for all of them. A field left off a
    /// page is not shown, checked or saved there. In a file, `context:` or
    /// the key's suffix: `notes@update`.
    pub context: Vec<FormContext>,
    /// The value a new record's form opens with: text, a number, a yes or
    /// no, or a list for a field that holds several.
    pub default: Option<serde_json::Value>,
    /// Another field whose starting value this one opens with, when it has
    /// no `default` of its own.
    pub default_from: Option<String>,
    /// Shown with its value and not editable. A save keeps what is stored;
    /// a new record takes the field's `default`.
    pub read_only: bool,
    /// Shown and not editable, and left out of every save.
    pub disabled: bool,
    /// A permission the operator must hold to see the field or write it.
    pub permission: Option<String>,
    /// Fields this one follows: when one of them changes, this field is
    /// drawn again with the form's current values, so a field whose choices
    /// come from an [option source](crate::options::OptionSource) offers a
    /// fresh list.
    pub depends_on: Vec<String>,
    /// The tab the field is on, by its label. A field under `tabs` or
    /// `secondary_tabs` that names none takes the set's `default_tab`.
    pub tab: Option<Text>,
    /// Which part of the form the field is in. A field naming a `tab` among
    /// the form's own fields is on the primary tabs.
    pub area: FormArea,
    /// Changes when another field's state meets a condition.
    pub trigger: Option<Trigger>,
    /// What a value looks like: the shapes accepted, the one stored, and how a
    /// `link` column on the field shows it. See [`crate::format::FieldFormat`].
    pub format: Option<crate::format::FieldFormat>,
}

/// How a preset shapes the text it copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetShape {
    /// As typed.
    Exact,
    /// Lowercase words joined by hyphens: `hello-world`.
    #[default]
    Slug,
    /// A slug with a leading slash: `/hello-world`.
    Url,
    /// A file name: spaces to hyphens, the rest kept.
    File,
}

impl PresetShape {
    /// The word the page carries, for the island.
    pub fn as_str(self) -> &'static str {
        match self {
            PresetShape::Exact => "exact",
            PresetShape::Slug => "slug",
            PresetShape::Url => "url",
            PresetShape::File => "file",
        }
    }
}

/// Fills a field from another as it is typed, until the field is edited.
///
/// In YAML, the followed field's name alone (`preset: title`, a slug), or a
/// map: `preset: { field: title, type: url }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preset {
    /// The field followed as it is typed.
    pub field: String,
    /// How the followed value is shaped into this field.
    pub shape: PresetShape,
}

impl Preset {
    /// Returns a preset that follows `field`, shaped as `shape`.
    pub fn new(field: impl Into<String>, shape: PresetShape) -> Self {
        Self {
            field: field.into(),
            shape,
        }
    }
}

impl<'de> Deserialize<'de> for Preset {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Full {
            field: String,
            #[serde(rename = "type", default)]
            shape: PresetShape,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Field(String),
            Full(Full),
        }
        Ok(match Raw::deserialize(de)? {
            Raw::Field(field) => Preset::new(field, PresetShape::Slug),
            Raw::Full(full) => Preset::new(full.field, full.shape),
        })
    }
}

/// Changes a field when another field's state meets a condition.
///
/// ```yaml
/// send_at:
///   type: date
///   trigger: { action: show, field: is_delayed, condition: checked }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    /// What happens when the condition holds: `show`, `hide`, `enable`,
    /// `disable`, `empty`, `fill[value]`, or several joined by `|`.
    pub action: String,
    /// The field watched. `name[]` watches every value of a checklist.
    pub field: String,
    /// `checked`, `unchecked`, `value[x]`, `value[x][y]` (either), `value[]`
    /// (empty), `value[*]` (anything), `value[foo*]` (a prefix).
    pub condition: String,
}

impl Trigger {
    /// Returns a trigger that applies `action` when `field` meets `condition`.
    pub fn new(
        action: impl Into<String>,
        field: impl Into<String>,
        condition: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            field: field.into(),
            condition: condition.into(),
        }
    }

    /// Refuses an action or condition the island does not know.
    pub fn check(&self) -> Result<(), String> {
        for action in self.action.split('|') {
            let known = matches!(action, "show" | "hide" | "enable" | "disable" | "empty")
                || (action.starts_with("fill[") && action.ends_with(']'));
            if !known {
                return Err(format!(
                    "unknown trigger action `{action}`: show, hide, enable, disable, empty or fill[value]"
                ));
            }
        }
        let known = matches!(self.condition.as_str(), "checked" | "unchecked")
            || (self.condition.starts_with("value[") && self.condition.ends_with(']'));
        if !known {
            return Err(format!(
                "unknown trigger condition `{}`: checked, unchecked or value[..]",
                self.condition
            ));
        }
        Ok(())
    }

    /// The name of the field watched, without a `[]` suffix.
    pub fn watched(&self) -> &str {
        self.field.trim_end_matches("[]")
    }
}

/// The attributes a field's wrapper carries for its preset and trigger, each
/// led by a space, empty when it has neither.
pub(crate) fn behaviour_attrs(field: &FormField) -> String {
    fn escaped(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    let mut attrs = String::new();
    if let Some(preset) = &field.preset {
        attrs.push_str(&format!(
            r#" data-lat-preset="{}" data-lat-preset-type="{}""#,
            escaped(&preset.field),
            preset.shape.as_str()
        ));
    }
    for (name, value) in &field.container_attributes {
        attrs.push_str(&format!(r#" {name}="{}""#, escaped(value)));
    }
    if !field.depends_on.is_empty() {
        attrs.push_str(&format!(
            r#" data-lat-depends="{}""#,
            escaped(&field.depends_on.join(","))
        ));
    }
    if let Some(trigger) = &field.trigger {
        attrs.push_str(&format!(
            r#" data-lat-trigger-action="{}" data-lat-trigger-field="{}" data-lat-trigger-condition="{}""#,
            escaped(&trigger.action),
            escaped(&trigger.field),
            escaped(&trigger.condition)
        ));
    }
    attrs
}

/// The share of a row a field takes, in twelfths. Written in YAML as a
/// fraction (`1/2`, `1/3`, `2/3`, `1/4`, `3/4`), `full`, a count of columns
/// (`1` to `12`), or the words `half`, `third`, `quarter`, `left`, `right` and
/// `auto` (each a half). Fields flow left to right and wrap when a row is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span(u8);

impl Span {
    /// The whole row.
    pub const FULL: Span = Span(12);

    /// `columns` twelfths of the row, clamped to 1 to 12.
    pub fn columns(columns: u8) -> Self {
        Span(columns.clamp(1, 12))
    }

    /// The twelfths this span covers.
    pub fn width(self) -> u8 {
        self.0
    }

    /// Reads the YAML spellings.
    pub fn parse(text: &str) -> Result<Self, String> {
        let columns = match text.trim() {
            "full" | "12/12" => 12,
            "1/2" | "half" | "left" | "right" | "auto" => 6,
            "1/3" | "third" => 4,
            "2/3" => 8,
            "1/4" | "quarter" => 3,
            "3/4" => 9,
            other => match other.parse::<u8>() {
                Ok(n) if (1..=12).contains(&n) => n,
                _ => {
                    return Err(format!(
                        "unknown span `{other}`, expected full, 1/2, 1/3, 2/3, 1/4, 3/4 or 1 to 12"
                    ))
                }
            },
        };
        Ok(Span(columns))
    }

    /// The CSS class the field wrapper carries; none for a full row.
    fn class(self) -> String {
        if self.0 == 12 {
            String::new()
        } else {
            format!(" lat-field--{}", self.0)
        }
    }
}

impl Default for Span {
    fn default() -> Self {
        Span::FULL
    }
}

impl Serialize for Span {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            12 => serializer.serialize_str("full"),
            6 => serializer.serialize_str("1/2"),
            4 => serializer.serialize_str("1/3"),
            8 => serializer.serialize_str("2/3"),
            3 => serializer.serialize_str("1/4"),
            9 => serializer.serialize_str("3/4"),
            n => serializer.serialize_u8(n),
        }
    }
}

impl<'de> Deserialize<'de> for Span {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Columns(u8),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Columns(n) if (1..=12).contains(&n) => Ok(Span(n)),
            Raw::Columns(n) => Err(serde::de::Error::custom(format!(
                "unknown span `{n}`, expected 1 to 12"
            ))),
            Raw::Text(text) => Span::parse(&text).map_err(serde::de::Error::custom),
        }
    }
}

/// The wrapper classes a field's layout adds: its span, and a row break.
pub(crate) fn layout_classes(span: Span, break_row: bool) -> String {
    let mut classes = span.class();
    if break_row {
        classes.push_str(" lat-field--break");
    }
    classes
}

/// What Enter does in a form's single-line fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnterPolicy {
    /// Submits the form. Cmd/Ctrl+Enter submits from a textarea as well.
    #[default]
    Submit,
    /// Enter never submits; Cmd/Ctrl+Enter still does.
    Off,
}

/// Where the cursor goes when a form opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormFocus {
    /// A new record opens with the cursor in its first field. A refused save
    /// comes back with the cursor in the first field it refused.
    #[default]
    Auto,
    /// The cursor is left where the browser puts it.
    Off,
}

/// What Enter does in one field, over the form's [`EnterPolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldEnter {
    /// The form's rule.
    #[default]
    Submit,
    /// Nothing.
    Off,
    /// Focus moves to the next field.
    Next,
}

impl FieldEnter {
    /// The `data-lat-enter` value the field wrapper carries; empty for the
    /// form's rule.
    fn attribute(self) -> &'static str {
        match self {
            FieldEnter::Submit => "",
            FieldEnter::Off => "off",
            FieldEnter::Next => "next",
        }
    }
}

impl Default for FormConfig {
    /// Everything a descriptor file leaves to the resource level: the entity,
    /// the path and the id column are filled in when the file is read.
    fn default() -> Self {
        Self {
            entity: String::new(),
            title: Text::new(""),
            base_path: String::new(),
            id_field: "id".to_string(),
            fields: Vec::new(),
            tabs: None,
            secondary_tabs: None,
            enter: EnterPolicy::default(),
            confirm_leave: true,
            focus: FormFocus::default(),
            persist: None,
            timestamps: false,
            footprints: false,
            slugs: Default::default(),
            encrypt: Vec::new(),
            related: Default::default(),
            actions: Vec::new(),
            linked_record: None,
            after_save: AfterSave::default(),
            panels: Vec::new(),
            done_path: None,
        }
    }
}

impl Default for FormField {
    /// A text field, named by the key it is written under.
    fn default() -> Self {
        Self::of("", Text::new(""), "text")
    }
}

impl<'de> Deserialize<'de> for FormField {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct Entry;
        impl<'de> serde::de::Visitor<'de> for Entry {
            type Value = FormField;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a field: its own keys and its type's")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<FormField, A::Error> {
                let mut field = FormField::default();
                let theirs = crate::keyed::split_entry(map, |key, map| {
                    match key {
                        "name" => field.name = map.next_value()?,
                        "label" => field.label = map.next_value()?,
                        "type" => field.field_type = map.next_value()?,
                        "rules" => field.rules = map.next_value()?,
                        "translatable" => field.translatable = map.next_value()?,
                        "help" => field.help = map.next_value()?,
                        "help_above" => field.help_above = map.next_value()?,
                        "css_class" => field.css_class = map.next_value()?,
                        "container_attributes" => field.container_attributes = map.next_value()?,
                        "enter" => field.enter = map.next_value()?,
                        "span" => field.span = map.next_value()?,
                        "break" => field.break_row = map.next_value()?,
                        "preset" => field.preset = map.next_value()?,
                        "trigger" => field.trigger = map.next_value()?,
                        "format" => field.format = Some(map.next_value()?),
                        "hidden" => field.hidden = map.next_value()?,
                        "transient" => field.transient = map.next_value()?,
                        "related" => field.related = Some(map.next_value()?),
                        "default" => field.default = Some(map.next_value()?),
                        "default_from" => field.default_from = Some(map.next_value()?),
                        "read_only" => field.read_only = map.next_value()?,
                        "disabled" => field.disabled = map.next_value()?,
                        "permission" => field.permission = Some(map.next_value()?),
                        "depends_on" => {
                            field.depends_on = map.next_value::<OneOrMany<String>>()?.into()
                        }
                        "tab" => field.tab = Some(map.next_value()?),
                        "area" => field.area = map.next_value()?,
                        "context" => {
                            field.context = map.next_value::<OneOrMany<FormContext>>()?.into()
                        }
                        _ => return Ok(false),
                    }
                    Ok(true)
                })?;
                field.options = theirs;
                if let Some(trigger) = &field.trigger {
                    trigger.check().map_err(serde::de::Error::custom)?;
                }
                field.check_markup().map_err(serde::de::Error::custom)?;
                Ok(field)
            }

            /// A key written bare (`title:`) is a field with every default.
            fn visit_unit<E: serde::de::Error>(self) -> Result<FormField, E> {
                Ok(FormField::default())
            }
        }
        de.deserialize_map(Entry)
    }
}

impl Serialize for FormField {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = ser.serialize_map(None)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("label", &self.label)?;
        map.serialize_entry("type", &self.field_type)?;
        map.serialize_entry("rules", &self.rules)?;
        map.serialize_entry("translatable", &self.translatable)?;
        map.serialize_entry("help", &self.help)?;
        if let Some(text) = &self.help_above {
            map.serialize_entry("help_above", text)?;
        }
        if let Some(class) = &self.css_class {
            map.serialize_entry("css_class", class)?;
        }
        if !self.container_attributes.is_empty() {
            map.serialize_entry("container_attributes", &self.container_attributes)?;
        }
        map.serialize_entry("enter", &self.enter)?;
        map.serialize_entry("span", &self.span)?;
        map.serialize_entry("break", &self.break_row)?;
        if let Some(preset) = &self.preset {
            map.serialize_entry("preset", preset)?;
        }
        if self.hidden {
            map.serialize_entry("hidden", &true)?;
        }
        if self.transient {
            map.serialize_entry("transient", &true)?;
        }
        if let Some(related) = &self.related {
            map.serialize_entry("related", related)?;
        }
        if !self.context.is_empty() {
            map.serialize_entry("context", &self.context)?;
        }
        if let Some(default) = &self.default {
            map.serialize_entry("default", default)?;
        }
        if let Some(from) = &self.default_from {
            map.serialize_entry("default_from", from)?;
        }
        if !self.depends_on.is_empty() {
            map.serialize_entry("depends_on", &self.depends_on)?;
        }
        if self.read_only {
            map.serialize_entry("read_only", &true)?;
        }
        if self.disabled {
            map.serialize_entry("disabled", &true)?;
        }
        if let Some(permission) = &self.permission {
            map.serialize_entry("permission", permission)?;
        }
        if let Some(tab) = &self.tab {
            map.serialize_entry("tab", tab)?;
        }
        if self.area == FormArea::Secondary {
            map.serialize_entry("area", &self.area)?;
        }
        if let Some(trigger) = &self.trigger {
            map.serialize_entry("trigger", trigger)?;
        }
        if let Some(format) = &self.format {
            map.serialize_entry("format", format)?;
        }
        crate::keyed::serialize_theirs(&mut map, &self.options)?;
        map.end()
    }
}

impl crate::keyed::Keyed for FormField {
    fn key(&self) -> &str {
        &self.name
    }
    fn set_key(&mut self, key: String) {
        // `notes@update` is the column `notes` on the update page alone.
        match key.split_once('@') {
            Some((name, context)) => {
                self.name = name.to_string();
                if let Some(context) = FormContext::named(context) {
                    if !self.context.contains(&context) {
                        self.context.push(context);
                    }
                }
            }
            None => self.name = key,
        }
    }
}

/// The part of a form a field is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormArea {
    /// Above the tabs, or the whole form when it has none.
    #[default]
    Outside,
    /// On the form's tabs.
    Primary,
    /// On the second set of tabs, under the first.
    Secondary,
}

/// A set of tabs on a form: its own keys, and the fields on it.
///
/// ```yaml
/// tabs:
///   default_tab: Details
///   active_tab: Pricing
///   icons: { Details: file-text, Pricing: tag }
///   fields:
///     body:  { type: textarea }
///     price: { tab: Pricing, input: number }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct FormTabs {
    /// The tab for a field here that names none. Default `Misc`.
    pub default_tab: Option<Text>,
    /// The tab open when the form loads: its label, or its place counted
    /// from 1. Default: the first.
    pub active_tab: Option<String>,
    /// An icon from the icon set for a tab, by the tab's label.
    pub icons: std::collections::BTreeMap<String, String>,
    /// Whether a tab can be opened by the address's fragment, and writes it.
    /// Default `true`.
    pub linkable: bool,
    /// The tabs fill the height left on the page.
    pub stretch: bool,
    /// A class on the set's container.
    pub css_class: Option<String>,
    /// A class on a tab's pane, by the tab's label, or one for every pane.
    pub pane_css_class: PaneClass,
    /// An HTML id for a tab, by the tab's label, for a script to find it.
    pub identifiers: std::collections::BTreeMap<String, String>,
    /// The fields on the tabs, written as the form's own are.
    #[serde(
        deserialize_with = "crate::keyed::deserialize",
        serialize_with = "crate::keyed::serialize"
    )]
    pub fields: Vec<FormField>,
}

impl Default for FormTabs {
    fn default() -> Self {
        Self {
            default_tab: None,
            active_tab: None,
            icons: Default::default(),
            linkable: true,
            stretch: false,
            css_class: None,
            pane_css_class: PaneClass::default(),
            identifiers: Default::default(),
            fields: Vec::new(),
        }
    }
}

/// A class for every pane of a tab set, or one a tab by its label.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PaneClass {
    /// No class.
    #[default]
    None,
    /// The same class on every pane.
    All(String),
    /// A class for each tab named.
    Each(std::collections::BTreeMap<String, String>),
}

impl PaneClass {
    fn of(&self, tab: &str) -> &str {
        match self {
            PaneClass::None => "",
            PaneClass::All(class) => class,
            PaneClass::Each(map) => map.get(tab).map(String::as_str).unwrap_or(""),
        }
    }
}

/// A page a form is shown on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormContext {
    /// The page that makes a record, `{base_path}/new`.
    Create,
    /// The page that changes one, `{base_path}/{id}/edit`.
    Update,
    /// The page that shows one, `{base_path}/{id}/preview`.
    Preview,
}

impl FormContext {
    fn named(name: &str) -> Option<Self> {
        match name.trim() {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "preview" => Some(Self::Preview),
            _ => None,
        }
    }
}

/// `context: update` or `context: [create, update]`.
#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> From<OneOrMany<T>> for Vec<T> {
    fn from(value: OneOrMany<T>) -> Self {
        match value {
            OneOrMany::One(one) => vec![one],
            OneOrMany::Many(many) => many,
        }
    }
}

impl FormField {
    /// Keeps the field to the pages named; see [`FormField::context`].
    pub fn context(mut self, pages: impl IntoIterator<Item = FormContext>) -> Self {
        self.context = pages.into_iter().collect();
        self
    }

    /// Opens a new record's form with `value` in the field.
    pub fn default_value(mut self, value: impl Into<serde_json::Value>) -> Self {
        self.default = Some(value.into());
        self
    }

    /// Opens a new record's form with the starting value of the field `other`.
    pub fn default_from(mut self, other: impl Into<String>) -> Self {
        self.default_from = Some(other.into());
        self
    }

    /// Shows the field with its value, not editable: a save keeps what is
    /// stored, and a new record takes the field's default.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Shows the field, not editable, and leaves it out of every save.
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }

    /// Shows the field to, and takes it from, only an operator holding
    /// `permission`.
    pub fn require(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }

    /// Whether an operator may change the field at all.
    pub(crate) fn locked(&self) -> bool {
        self.read_only || self.disabled
    }

    /// Draws the field again whenever one of `fields` changes.
    pub fn depends_on<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.depends_on = fields.into_iter().map(Into::into).collect();
        self
    }

    /// Puts the field on the form's tab labelled `tab`.
    pub fn tab(mut self, tab: impl Into<Text>) -> Self {
        self.tab = Some(tab.into());
        self.area = FormArea::Primary;
        self
    }

    /// Whether the field is on `page`.
    pub fn on(&self, page: FormContext) -> bool {
        self.context.is_empty() || self.context.contains(&page)
    }

    /// A field of a registered type by key.
    pub fn of(name: &str, label: impl Into<Text>, field_type: &str) -> Self {
        Self {
            name: name.to_string(),
            label: label.into(),
            field_type: field_type.to_string(),
            options: serde_json::Value::Null,
            rules: Vec::new(),
            translatable: false,
            help: None,
            help_above: None,
            css_class: None,
            container_attributes: Default::default(),
            enter: FieldEnter::default(),
            span: Span::default(),
            break_row: false,
            preset: None,
            trigger: None,
            hidden: false,
            transient: false,
            related: None,
            context: Vec::new(),
            default: None,
            default_from: None,
            depends_on: Vec::new(),
            read_only: false,
            disabled: false,
            permission: None,
            tab: None,
            area: FormArea::Outside,
            format: None,
        }
    }

    /// Accepts and stores values in `format`.
    pub fn format(mut self, format: crate::format::FieldFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Follows `field` as it is typed, shaped as `shape`, until this field is
    /// edited.
    pub fn preset(mut self, field: &str, shape: PresetShape) -> Self {
        self.preset = Some(Preset::new(field, shape));
        self
    }

    /// Changes when `trigger`'s condition holds on the field it watches.
    pub fn trigger(mut self, trigger: Trigger) -> Self {
        self.trigger = Some(trigger);
        self
    }

    /// Returns a single-line text field over the column `name`.
    pub fn text(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "text")
    }

    /// Returns a multi-line text field over the column `name`.
    pub fn textarea(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "textarea")
    }

    /// A reference field: a picker over a registered picker source (a `vendor.name`
    /// key), which the field searches and resolves against. Builds the `source`
    /// option so a descriptor needs no raw JSON.
    pub fn reference(name: &str, label: impl Into<Text>, source: &str) -> Self {
        Self::of(name, label, "reference").options(serde_json::json!({ "source": source }))
    }

    /// A dropdown over a fixed set of `(value, label)` choices.
    pub fn select(name: &str, label: impl Into<Text>, options: Vec<(&str, &str)>) -> Self {
        Self::of(name, label, "select").options(Self::choices(options))
    }

    /// The same choices rendered as radio buttons.
    pub fn radio(name: &str, label: impl Into<Text>, options: Vec<(&str, &str)>) -> Self {
        Self::of(name, label, "radio").options(Self::choices(options))
    }

    /// A checkbox over a boolean column.
    pub fn switch(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "switch")
    }

    /// A list of short texts typed one at a time, stored as a JSON array.
    pub fn taglist(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "taglist")
    }

    /// A calendar date, stored as `YYYY-MM-DD`.
    pub fn date(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "date")
    }

    /// A password: hashed on save, never rendered back, and left alone when the
    /// field is submitted blank on an edit.
    pub fn password(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "password")
    }

    /// A list of rows, each holding `fields`, stored as an array of objects.
    pub fn repeater(name: &str, label: impl Into<Text>, fields: Vec<FormField>) -> Self {
        Self::of(name, label, "repeater").options(serde_json::json!({ "fields": fields }))
    }

    /// A repeater whose rows collapse to the line that names them, opening one
    /// at a time. `summary_field` names the sub-field that titles a row; unset,
    /// the first one does.
    ///
    /// Use it when a row holds more than one or two fields: rendered inline
    /// those become columns, and a row of four is already unreadable. A single
    /// narrow column is better left inline, where collapsing would hide the one
    /// thing worth seeing.
    pub fn repeater_list(
        name: &str,
        label: impl Into<Text>,
        fields: Vec<FormField>,
        summary_field: Option<&str>,
    ) -> Self {
        Self::of(name, label, "repeater").options(serde_json::json!({
            "fields": fields,
            "display": "list",
            "summary_field": summary_field,
        }))
    }

    /// The options blob the choice-shaped types read.
    fn choices(options: Vec<(&str, &str)>) -> serde_json::Value {
        let options: Vec<serde_json::Value> = options
            .into_iter()
            .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
            .collect();
        serde_json::json!({ "options": options })
    }

    /// Sets the field type's typed options (the type validates them at render).
    pub fn options(mut self, options: serde_json::Value) -> Self {
        self.options = options;
        self
    }

    /// Requires a non-empty value in every mode.
    pub fn required(mut self) -> Self {
        self.rules.push(Rule::Required);
        self
    }

    /// Requires a non-empty value in one mode only (for example a password set on
    /// create but left unchanged on edit).
    pub fn required_on(mut self, mode: Mode) -> Self {
        self.rules.push(Rule::RequiredOn(mode));
        self
    }

    /// Requires the value to be unique in this field's column (a DB probe that
    /// ignores the edited row on update).
    pub fn unique(mut self) -> Self {
        self.rules.push(Rule::Unique);
        self
    }

    /// Caps the value's length.
    pub fn max_length(mut self, n: usize) -> Self {
        self.rules.push(Rule::MaxLength(n));
        self
    }

    /// Requires at least `n` characters when the value is non-empty.
    pub fn min_length(mut self, n: usize) -> Self {
        self.rules.push(Rule::MinLength(n));
        self
    }

    /// Requires a syntactically valid email address.
    pub fn email(mut self) -> Self {
        self.rules.push(Rule::Email);
        self
    }

    /// Marks this field as translatable content (see [`FormField::translatable`]).
    pub fn translatable(mut self) -> Self {
        self.translatable = true;
        self
    }

    /// Help text shown beneath the control.
    pub fn help(mut self, text: impl Into<Text>) -> Self {
        self.help = Some(text.into());
        self
    }

    /// Shows `text` above the control, under the label.
    pub fn help_above(mut self, text: impl Into<Text>) -> Self {
        self.help_above = Some(text.into());
        self
    }

    /// Adds `class` (one or several, space separated) to the field's wrapper.
    pub fn css_class(mut self, class: impl Into<String>) -> Self {
        self.css_class = Some(class.into());
        self
    }

    /// Adds an attribute to the field's wrapper.
    pub fn container_attribute(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.container_attributes.insert(name.into(), value.into());
        self
    }

    /// Refuses a class or an attribute name that is not plain markup: a class
    /// of letters, digits, `-` and `_`; an attribute named likewise, and
    /// never an event handler, `class`, `style` or one the framework sets.
    pub(crate) fn check_markup(&self) -> Result<(), String> {
        let plain = |text: &str| {
            !text.is_empty()
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        if let Some(class) = &self.css_class {
            if !class.split_whitespace().all(plain) {
                return Err(format!(
                    "`css_class` holds `{class}`, which is not a list of class names"
                ));
            }
        }
        for name in self.container_attributes.keys() {
            let lower = name.to_ascii_lowercase();
            if !plain(name)
                || lower.starts_with("on")
                || lower.starts_with("data-lat-")
                || matches!(lower.as_str(), "class" | "style" | "id" | "hidden")
            {
                return Err(format!("`container_attributes` cannot set `{name}`"));
            }
        }
        Ok(())
    }

    /// The wrapper's classes: the span and row break, then the field's own.
    pub(crate) fn wrapper_classes(&self) -> String {
        let mut classes = layout_classes(self.span, self.break_row);
        for class in self.css_class.iter().flat_map(|c| c.split_whitespace()) {
            classes.push(' ');
            classes.push_str(class);
        }
        classes
    }

    /// `control` with the field's text above it, when it has any.
    pub(crate) fn with_help_above(&self, control: String, shell: &crate::Shell) -> String {
        match &self.help_above {
            Some(text) => format!(
                r#"<p class="lat-field__help lat-field__help--above">{}</p>{control}"#,
                escape_attr(&shell.tt(text))
            ),
            None => control,
        }
    }

    /// The share of the row this field takes.
    pub fn span(mut self, span: Span) -> Self {
        self.span = span;
        self
    }

    /// Starts a new row.
    pub fn break_row(mut self) -> Self {
        self.break_row = true;
        self
    }

    /// Marks the field transient: shown and submitted, never loaded from or
    /// written to a column. The form's persister handles its value.
    pub fn transient(mut self) -> Self {
        self.transient = true;
        self
    }
}

/// A button beside Save on an existing record's form.
///
/// `action` is a path under the admin root with `{id}` standing for the
/// record's id. A `post` action submits the form there, request token
/// included, so the handler also sees what the operator typed; a `get` action
/// is a link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct FormAction {
    /// The button's words.
    pub label: Text,
    /// A path under the admin root, `{id}` replaced by the record's id.
    pub action: String,
    /// `post` (the default) or `get`.
    #[serde(default = "post_method")]
    pub method: String,
    /// A question asked before the action runs.
    #[serde(default)]
    pub confirm: Option<Text>,
    /// A permission the operator must hold to see the button.
    #[serde(default)]
    pub permission: Option<String>,
}

impl FormAction {
    /// A `post` action labelled `label` at `action`.
    pub fn new(label: impl Into<Text>, action: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
            method: post_method(),
            confirm: None,
            permission: None,
        }
    }

    /// Makes it a link rather than a submission.
    pub fn link(mut self) -> Self {
        self.method = "get".into();
        self
    }

    /// Asks `question` before running.
    pub fn confirm(mut self, question: impl Into<Text>) -> Self {
        self.confirm = Some(question.into());
        self
    }

    /// Shows the button only to an operator holding `code`.
    pub fn permission(mut self, code: impl Into<String>) -> Self {
        self.permission = Some(code.into());
        self
    }
}

fn post_method() -> String {
    "post".into()
}

/// A form descriptor: which table, its editable fields, the id column, and the
/// base path the form lives under (`{base_path}/new`, `{base_path}/{id}/edit`).
///
/// Built with [`FormConfig::new`] plus its builder methods. Non-exhaustive so the
/// framework can learn something new about a form after 1.0 without a major bump.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct FormConfig {
    /// The table the form reads and writes.
    pub entity: String,
    /// The screen title, localized at render. Serde stays a plain string.
    pub title: Text,
    /// The path the form lives under (`{base_path}/new`, `{base_path}/{id}/edit`).
    pub base_path: String,
    /// The primary-key column records are identified by (`id`).
    pub id_field: String,
    #[serde(
        deserialize_with = "crate::keyed::deserialize",
        serialize_with = "crate::keyed::serialize"
    )]
    /// The fields the form shows, in order.
    pub fields: Vec<FormField>,
    /// The form's tabs and the fields on them; see [`FormTabs`].
    pub tabs: Option<FormTabs>,
    /// A second set of tabs, under the first.
    pub secondary_tabs: Option<FormTabs>,
    /// What Enter does in the form's single-line fields. Default `submit`.
    #[serde(default)]
    pub enter: EnterPolicy,
    /// Ask before leaving the form with changes unsaved. Default `true`.
    pub confirm_leave: bool,
    /// Where the cursor goes when the form opens. Default `auto`.
    pub focus: FormFocus,
    /// The registered persister that writes this form (a dotted `vendor.name`),
    /// or `None` for the built-in descriptor insert/update. See [`crate::persist`].
    pub persist: Option<String>,
    /// Stamp `created_at` and `updated_at` on write. Opt in per form, because
    /// the columns have to exist on this entity's table.
    #[serde(default)]
    pub timestamps: bool,
    /// Record who created and last changed each row in `created_user_id` and
    /// `updated_user_id`. Opt in per form, like `timestamps`.
    #[serde(default)]
    pub footprints: bool,
    /// Slug columns filled when empty, each from one attribute or a list joined
    /// in order (`slug: title`, `slug: [first_name, last_name]`).
    #[serde(default)]
    pub slugs: std::collections::BTreeMap<String, SlugSources>,
    /// Columns sealed under the application key on every save through a form
    /// or an import.
    #[serde(default)]
    pub encrypt: Vec<String>,
    /// One-to-one records whose fields this form edits, by name: a field with
    /// `related: profile` is stored on that record, saved in the same
    /// transaction as this one.
    #[serde(default)]
    pub related: std::collections::BTreeMap<String, RelatedRecord>,
    /// Buttons beside Save on an existing record's form, for a module's own
    /// actions on it: Reset, Duplicate, Publish.
    #[serde(default)]
    pub actions: Vec<FormAction>,
    /// The record of any resource this one belongs to, linked above the form.
    #[serde(default)]
    pub linked_record: Option<LinkedRecord>,
    /// Where a save goes when it is not the list. See [`AfterSave`].
    #[serde(default)]
    pub after_save: AfterSave,
    /// Panels a module draws under an existing record's form, each fetched
    /// once the page is open: the same shape a preview's panels take.
    #[serde(default)]
    pub panels: Vec<PreviewPanel>,
    /// Where a save and a cancel return to, when it is not the list under
    /// `base_path`: a related record's form returns to its parent. Set by the
    /// router, never by a descriptor.
    #[serde(skip)]
    pub(crate) done_path: Option<String>,
}

impl FormConfig {
    /// A form over `entity`, mounted under `base_path`, keyed by `id_field`.
    ///
    /// The four arguments are the ones with no sensible default; everything else
    /// is opt-in through a builder method, so a later field costs nothing.
    pub fn new(
        entity: impl Into<String>,
        title: impl Into<Text>,
        base_path: impl Into<String>,
        id_field: impl Into<String>,
        fields: Vec<FormField>,
    ) -> Self {
        Self {
            enter: Default::default(),
            confirm_leave: true,
            focus: FormFocus::default(),
            entity: entity.into(),
            title: title.into(),
            base_path: base_path.into(),
            id_field: id_field.into(),
            fields,
            tabs: None,
            secondary_tabs: None,
            persist: None,
            timestamps: false,
            footprints: false,
            slugs: Default::default(),
            encrypt: Vec::new(),
            related: Default::default(),
            actions: Vec::new(),
            linked_record: None,
            after_save: AfterSave::default(),
            panels: Vec::new(),
            done_path: None,
        }
    }

    /// Writes this form through a registered persister instead of the built-in
    /// descriptor insert/update.
    pub fn persist(mut self, name: impl Into<String>) -> Self {
        self.persist = Some(name.into());
        self
    }

    /// Whether leaving the form with changes unsaved asks first. Default `true`.
    pub fn confirm_leave(mut self, ask: bool) -> Self {
        self.confirm_leave = ask;
        self
    }

    /// Where the cursor goes when the form opens.
    pub fn focus(mut self, focus: FormFocus) -> Self {
        self.focus = focus;
        self
    }

    /// Stamps `created_at` and `updated_at` on write. Opt in, because the
    /// columns have to exist on this entity's table.
    pub fn timestamps(mut self) -> Self {
        self.timestamps = true;
        self
    }

    /// Records who created and last changed each row. Opt in, because the
    /// `created_user_id` and `updated_user_id` columns have to exist.
    pub fn footprints(mut self) -> Self {
        self.footprints = true;
        self
    }

    /// Fills `column` from `sources` when it is empty, unique within the table.
    pub fn slug(mut self, column: impl Into<String>, sources: &[&str]) -> Self {
        self.slugs.insert(
            column.into(),
            SlugSources::Many(sources.iter().map(|s| s.to_string()).collect()),
        );
        self
    }

    /// Seals `columns` under the application key on every write.
    pub fn encrypt(mut self, columns: &[&str]) -> Self {
        self.encrypt = columns.iter().map(|c| c.to_string()).collect();
        self
    }

    fn idents_valid(&self) -> bool {
        valid_ident(&self.entity)
            && valid_ident(&self.id_field)
            && self.fields.iter().all(|f| valid_ident(&f.name))
            && self
                .slugs
                .iter()
                .all(|(c, s)| valid_ident(c) && s.list().iter().all(|n| valid_ident(n)))
            && self.encrypt.iter().all(|c| valid_ident(c))
            && self
                .linked_record
                .as_ref()
                .is_none_or(|l| valid_ident(&l.morph))
            && self
                .related
                .iter()
                .all(|(n, r)| valid_ident(n) && r.idents_valid())
            && self.fields.iter().all(|f| {
                f.related
                    .as_ref()
                    .is_none_or(|name| self.related.contains_key(name))
            })
    }
}

/// The record a form's record belongs to, when that record may be of any
/// resource: a pair of columns named `{morph}_type` (the owner's entity) and
/// `{morph}_id` (its id). An edit page links the owner above the form, by the
/// first column of the owner resource's list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct LinkedRecord {
    /// The name `n` of the pair of columns `n_type` and `n_id`.
    pub morph: String,
    /// What the link is called. Default "Belongs to".
    pub label: Option<Text>,
    /// What stands in for the link when the columns are empty. Default
    /// "Not linked".
    pub empty: Option<Text>,
}

impl LinkedRecord {
    /// A link to the record named by the columns `{morph}_type` and `{morph}_id`.
    pub fn morph(morph: impl Into<String>) -> Self {
        Self {
            morph: morph.into(),
            label: None,
            empty: None,
        }
    }

    /// Sets what the link is called.
    pub fn label(mut self, label: impl Into<Text>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets what stands in for the link when the record belongs to nothing.
    pub fn empty(mut self, empty: impl Into<Text>) -> Self {
        self.empty = Some(empty.into());
        self
    }
}

impl FormConfig {
    /// Links the record this one belongs to above the form: see
    /// [`LinkedRecord`].
    pub fn linked_record(mut self, linked: LinkedRecord) -> Self {
        self.linked_record = Some(linked);
        self
    }
}

/// The linked record as the page shows it.
pub(crate) struct LinkedView {
    label: String,
    text: String,
    href: Option<String>,
}

/// Reads the record `id` belongs to and where it is edited. The link is left
/// off for an owner whose resource `permissions` cannot see, and for an entity
/// no resource shows.
async fn linked_view(
    state: &AdminState,
    form: &FormConfig,
    id: &str,
    permissions: &laterite_auth::PermissionSet,
    shell: &crate::Shell,
) -> Option<LinkedView> {
    let linked = form.linked_record.as_ref()?;
    let cast = || Alias::new(text_cast(state.db.backend));
    let (sql, values) = to_sql(
        state.db.backend,
        Query::select()
            .expr_as(
                Expr::col(Alias::new(format!("{}_type", linked.morph))).cast_as(cast()),
                Alias::new("lat_type"),
            )
            .expr_as(
                Expr::col(Alias::new(format!("{}_id", linked.morph))).cast_as(cast()),
                Alias::new("lat_id"),
            )
            .from(Alias::new(&form.entity))
            .and_where(Expr::col(Alias::new(&form.id_field)).cast_as(cast()).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&state.db.pool)
        .await
        .ok()??;
    let text_of = |name: &str| {
        laterite_core::AnyRowExt::get_text_opt(&row, name)
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
    };
    let label = shell.tt(&linked.label.clone().unwrap_or_else(|| t!("Belongs to")));
    let (Some(owner_type), Some(owner_id)) = (text_of("lat_type"), text_of("lat_id")) else {
        return Some(LinkedView {
            label,
            text: shell.tt(&linked.empty.clone().unwrap_or_else(|| t!("Not linked"))),
            href: None,
        });
    };
    let fallback = format!("{owner_type} #{owner_id}");
    let target = state.entity_links.get(&owner_type).filter(|t| {
        t.permission
            .as_deref()
            .is_none_or(|p| permissions.allows(p))
    });
    let Some(target) = target else {
        return Some(LinkedView {
            label,
            text: fallback,
            href: None,
        });
    };
    let name = match target.label.as_deref() {
        Some(column)
            if valid_ident(column) && valid_ident(&owner_type) && valid_ident(&target.id_field) =>
        {
            let (sql, values) = to_sql(
                state.db.backend,
                Query::select()
                    .expr_as(
                        Expr::col(Alias::new(column)).cast_as(cast()),
                        Alias::new("lat_label"),
                    )
                    .from(Alias::new(&owner_type))
                    .and_where(
                        Expr::col(Alias::new(&target.id_field))
                            .cast_as(cast())
                            .eq(owner_id.as_str()),
                    )
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .fetch_optional(&state.db.pool)
                .await
                .ok()
                .flatten()
                .and_then(|row| {
                    laterite_core::AnyRowExt::get_text_opt(&row, "lat_label")
                        .ok()
                        .flatten()
                })
                .filter(|v| !v.is_empty())
        }
        _ => None,
    };
    Some(LinkedView {
        label,
        text: format!("{}: {}", shell.tt(&target.title), name.unwrap_or(fallback)),
        href: Some(format!("{}/{}/edit", target.base_path, owner_id)),
    })
}

/// A one-to-one record a form edits beside its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct RelatedRecord {
    /// The related table.
    pub entity: String,
    /// Has-one: the column on the related table holding this record's id.
    pub foreign_key: Option<String>,
    /// Belongs-to: the column on this record's table holding the related id.
    pub belongs_to: Option<String>,
    /// The related table's id column. Default `id`.
    pub id_field: Option<String>,
    /// Belongs-to: with every field of it left empty no record is made, and
    /// one already there is let go (this record's column is cleared; the
    /// related row stays).
    pub optional: bool,
    /// Stamps the related row's `created_at` and `updated_at` as it is
    /// written.
    pub timestamps: bool,
}

impl RelatedRecord {
    /// A has-one record: rows of `entity` whose `foreign_key` holds this id.
    pub fn has_one(entity: impl Into<String>, foreign_key: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            foreign_key: Some(foreign_key.into()),
            ..Self::default()
        }
    }

    /// A belongs-to record: the row of `entity` this record's `column` names.
    pub fn belongs_to(entity: impl Into<String>, column: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            belongs_to: Some(column.into()),
            ..Self::default()
        }
    }

    /// Stamps the related row's `created_at` and `updated_at`.
    pub fn timestamps(mut self) -> Self {
        self.timestamps = true;
        self
    }

    /// Makes a belongs-to record optional: empty fields make no record.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    pub(crate) fn id_field(&self) -> &str {
        self.id_field.as_deref().unwrap_or("id")
    }

    pub(crate) fn idents_valid(&self) -> bool {
        valid_ident(&self.entity)
            && self.foreign_key.as_deref().is_none_or(valid_ident)
            && self.belongs_to.as_deref().is_none_or(valid_ident)
            && valid_ident(self.id_field())
            && (self.foreign_key.is_some() != self.belongs_to.is_some())
    }
}

/// The attributes a slug is built from: one name or a list joined in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SlugSources {
    /// A single attribute.
    One(String),
    /// Several attributes, joined with spaces before slugging.
    Many(Vec<String>),
}

impl SlugSources {
    /// The attribute names, in order.
    pub fn list(&self) -> Vec<String> {
        match self {
            Self::One(name) => vec![name.clone()],
            Self::Many(names) => names.clone(),
        }
    }
}

/// A [`FormConfig`] with each field's options resolved once, at router build.
/// Handlers carry this so option resolution and intrinsic-rule merging happen a
/// single time (not per render), and a malformed option or unregistered type
/// aborts boot rather than degrading silently to plain text.
#[derive(Clone)]
pub(crate) struct PreparedForm {
    pub(crate) config: FormConfig,
    /// Parallel to `config.fields`.
    fields: Vec<PreparedField>,
    /// The write handler: the named persister, or the built-in default.
    persister: Arc<dyn Persister>,
    /// Listeners for this form's entity, in registration order.
    listeners: Vec<Arc<dyn ModelListener>>,
    /// Offer "Save and continue editing" beside Save: a resource with relation
    /// sections wants to land on the edit page, where the sections are.
    pub(crate) stay: bool,
    /// The grant a save asks for beside the resource's; without it the edit
    /// page opens read-only.
    pub(crate) update_permission: Option<String>,
    /// Whether the record has an edit page: a preview on a resource with no
    /// form offers none.
    pub(crate) editable: bool,
    /// The registered validation rules a field may name.
    rules: Arc<laterite_core::validation::RuleRegistry>,
}

impl PreparedForm {
    /// What a new record's form opens with: each field's `default`, as its
    /// control shows it, then each `default_from` from the field it names.
    /// Text is translated, since a default may be a phrase.
    pub(crate) fn defaults(&self, shell: &crate::Shell) -> HashMap<String, String> {
        let mut values = HashMap::new();
        for (field, prepared) in self.config.fields.iter().zip(&self.fields) {
            let Some(default) = &field.default else {
                continue;
            };
            let text = match default {
                serde_json::Value::Null => continue,
                serde_json::Value::String(text) => shell.t(text),
                other => other.to_string(),
            };
            values.insert(
                field.name.clone(),
                prepared
                    .field_type
                    .to_control_for(&field.name, Some(&text), &prepared.opts),
            );
        }
        for field in &self.config.fields {
            if field.default.is_some() {
                continue;
            }
            if let Some(value) = field
                .default_from
                .as_ref()
                .and_then(|from| values.get(from))
            {
                values.insert(field.name.clone(), value.clone());
            }
        }
        values
    }
}

/// One field's boot-resolved state: its typed options and its merged rules (the
/// type's intrinsic rules ahead of the descriptor's own).
#[derive(Clone)]
struct PreparedField {
    /// Resolved once at boot, so the save and load hooks need no lookup.
    field_type: Arc<dyn crate::field::FieldType>,
    opts: ResolvedOptions,
    rules: Vec<Rule>,
    /// The option source the field's choices come from, when it names one.
    source: Option<String>,
}

/// Refuses a field following one the form lacks, itself, or a field that
/// follows it back.
fn check_dependencies(fields: &[FormField]) -> Result<(), String> {
    for field in fields {
        for followed in &field.depends_on {
            if followed == &field.name {
                return Err(format!("field `{}` depends on itself", field.name));
            }
            if !fields.iter().any(|other| &other.name == followed) {
                return Err(format!(
                    "field `{}` depends on `{followed}`, which is not in this form",
                    field.name
                ));
            }
        }
    }
    // Walk what each field follows; meeting the start again is a loop.
    for start in fields {
        let mut seen: Vec<&str> = Vec::new();
        let mut queue: Vec<&str> = start.depends_on.iter().map(String::as_str).collect();
        while let Some(name) = queue.pop() {
            if name == start.name {
                return Err(format!(
                    "field `{}` depends on itself through other fields",
                    start.name
                ));
            }
            if seen.contains(&name) {
                continue;
            }
            seen.push(name);
            if let Some(field) = fields.iter().find(|f| f.name == name) {
                queue.extend(field.depends_on.iter().map(String::as_str));
            }
        }
    }
    Ok(())
}

impl PreparedForm {
    /// The form keeping only the fields `keep` accepts, or itself when it
    /// accepts them all.
    fn keeping<'a>(
        &'a self,
        keep: impl Fn(&FormField) -> bool,
    ) -> std::borrow::Cow<'a, PreparedForm> {
        if self.config.fields.iter().all(&keep) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut form = self.clone();
        let kept: Vec<bool> = self.config.fields.iter().map(&keep).collect();
        let mut at = kept.iter();
        form.config.fields.retain(|_| *at.next().unwrap_or(&true));
        let mut at = kept.iter();
        form.fields.retain(|_| *at.next().unwrap_or(&true));
        std::borrow::Cow::Owned(form)
    }

    /// The form as an operator may see it: without the fields whose
    /// permission `allows` does not grant.
    pub(crate) fn seen_by<'a>(
        &'a self,
        allows: impl Fn(&str) -> bool,
    ) -> std::borrow::Cow<'a, PreparedForm> {
        self.keeping(|field| field.permission.as_deref().is_none_or(&allows))
    }

    /// The fields a save writes: none that is disabled, and on an update none
    /// that is read-only, so a submission cannot change what the page locked.
    pub(crate) fn written<'a>(&'a self, update: bool) -> std::borrow::Cow<'a, PreparedForm> {
        self.keeping(|field| !field.disabled && !(update && field.read_only))
    }

    /// The permissions the form's fields name.
    pub(crate) fn field_permissions(&self) -> impl Iterator<Item = (&str, &str)> {
        self.config.fields.iter().filter_map(|field| {
            field
                .permission
                .as_deref()
                .map(|permission| (field.name.as_str(), permission))
        })
    }

    /// Refuses a field naming an option source nobody registered.
    pub(crate) fn check_sources(
        &self,
        sources: &crate::options::OptionRegistry,
    ) -> Result<(), String> {
        for (field, prepared) in self.config.fields.iter().zip(&self.fields) {
            if let Some(source) = &prepared.source {
                if !sources.contains_key(source) {
                    return Err(format!(
                        "field `{}` takes its options from `{source}`, which no module registered",
                        field.name
                    ));
                }
            }
        }
        Ok(())
    }

    /// The form with each sourced field's choices asked of its source, given
    /// `values`: what a request renders and saves through. A form with no
    /// sourced field is returned as it is.
    pub(crate) async fn loaded<'a>(
        &'a self,
        state: &AdminState,
        values: &HashMap<String, String>,
        id: Option<&str>,
        shell: &crate::Shell,
    ) -> std::borrow::Cow<'a, PreparedForm> {
        if self.fields.iter().all(|pf| pf.source.is_none()) {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut form = self.clone();
        // What each control shows, which is what a source is asked with: a
        // dropdown holding nothing shows its first choice.
        let mut values = values.clone();
        let shown = |values: &mut HashMap<String, String>, index: usize, form: &PreparedForm| {
            let name = &form.config.fields[index].name;
            let prepared = &form.fields[index];
            let current = values.get(name).map(String::as_str).unwrap_or("");
            if let Some(value) = prepared.field_type.shown(current, &prepared.opts) {
                values.insert(name.clone(), value);
            }
        };
        let mut pending = Vec::new();
        for (index, prepared) in self.fields.iter().enumerate() {
            if prepared.source.is_some() {
                pending.push(index);
            } else {
                shown(&mut values, index, &form);
            }
        }
        // A field is asked once every field it follows has its choices.
        while !pending.is_empty() {
            let ready = pending
                .iter()
                .position(|&index| {
                    !self.config.fields[index].depends_on.iter().any(|followed| {
                        pending.iter().any(|&other| {
                            other != index && &self.config.fields[other].name == followed
                        })
                    })
                })
                .unwrap_or(0);
            let index = pending.remove(ready);
            let field = &self.config.fields[index];
            let prepared = &self.fields[index];
            let Some(name) = &prepared.source else {
                continue;
            };
            let entries = {
                let cx = crate::options::OptionCx {
                    field: &field.name,
                    value: values.get(&field.name).map(String::as_str).unwrap_or(""),
                    values: &values,
                    id,
                    i18n: shell.i18n(),
                };
                match state.option_sources.get(name) {
                    Some(reg) => match reg.source.options(&state.db, &cx).await {
                        Ok(entries) => entries,
                        Err(error) => {
                            tracing::error!(source = %name, field = %field.name, %error, "option source failed");
                            Vec::new()
                        }
                    },
                    None => {
                        tracing::error!(source = %name, field = %field.name, "option source is not registered");
                        Vec::new()
                    }
                }
            };
            form.fields[index].opts = prepared.field_type.with_options(&prepared.opts, entries);
            shown(&mut values, index, &form);
        }
        std::borrow::Cow::Owned(form)
    }

    /// The write handler this form resolved to.
    pub(crate) fn persister(&self) -> Arc<dyn Persister> {
        self.persister.clone()
    }

    /// Whether the form declares `field`.
    pub(crate) fn has_field(&self, field: &str) -> bool {
        self.config.fields.iter().any(|f| f.name == field)
    }

    /// The widget assets `field`'s type declares.
    pub(crate) fn field_assets(&self, field: &str) -> Vec<&'static str> {
        self.config
            .fields
            .iter()
            .zip(&self.fields)
            .find(|(f, _)| f.name == field)
            .map(|(_, pf)| pf.field_type.assets(&pf.opts))
            .unwrap_or_default()
    }

    /// One field's bare control holding `value`, as the form would render it,
    /// for a place outside the form such as an editable cell. `None` when the
    /// form has no such field.
    pub(crate) fn control(
        &self,
        state: &AdminState,
        field: &str,
        dom_id: &str,
        value: &str,
        shell: &crate::Shell,
    ) -> Option<String> {
        let (f, pf) = self
            .config
            .fields
            .iter()
            .zip(&self.fields)
            .find(|(f, _)| f.name == field)?;
        let value = FieldValue::Text(value.to_string());
        let label = shell.tt(&f.label);
        let cx = FieldCx {
            name: &f.name,
            id: dom_id,
            label: &label,
            value: &value,
            required: has_required(&pf.rules),
            max_length: max_length(&pf.rules),
            opts: &pf.opts,
            base: &shell.base,
            i18n: shell.i18n(),
            icons: crate::icons::Icons::new(&shell.icon_set, &shell.sprite_url),
        };
        let ft = state.field_types.get(&f.field_type)?;
        let scope = OverrideScope {
            surface: Surface::Field,
            view_key: &f.field_type,
            resource: Some(&self.config.base_path),
            field: Some(&f.name),
        };
        Some(render_field(ft.as_ref(), state.overrides.as_ref(), &scope, &cx).into_string())
    }

    /// Resolves every field against the registry. Returns the offending field
    /// and reason (for a boot abort) when a type is unregistered or rejects its
    /// options.
    /// The form with the registered validation rules its fields may name.
    pub(crate) fn with_rules(
        mut self,
        rules: Arc<laterite_core::validation::RuleRegistry>,
    ) -> Self {
        self.rules = rules;
        self
    }

    pub(crate) fn prepare(
        config: FormConfig,
        field_types: &crate::field::FieldRegistry,
        persisters: &PersisterRegistry,
        listeners: &[ModelListenerReg],
    ) -> Result<Self, String> {
        let mut config = config;
        config.absorb_tabs();
        config.after_save.check()?;
        if !config.idents_valid() {
            return Err(format!(
                "form for `{}` names an invalid column (a field, slug, encrypted column or related record), or a field names a related record the form does not declare (each declares one of foreign_key and belongs_to)",
                config.entity
            ));
        }
        if let Some(f) = config
            .fields
            .iter()
            .find(|f| f.related.is_some() && f.rules.iter().any(|r| matches!(r, Rule::Unique)))
        {
            return Err(format!(
                "field `{}` is stored on a related record, where `unique` cannot check its table",
                f.name
            ));
        }
        let mut fields = Vec::with_capacity(config.fields.len());
        for f in &config.fields {
            let names = || config.fields.iter().map(|other| other.name.as_str());
            if let Some(preset) = &f.preset {
                if !names().any(|n| n == preset.field) {
                    return Err(format!(
                        "field `{}` presets from `{}`, which is not in this form",
                        f.name, preset.field
                    ));
                }
            }
            if let Some(trigger) = &f.trigger {
                trigger
                    .check()
                    .map_err(|e| format!("field `{}`: {e}", f.name))?;
                if !names().any(|n| n == trigger.watched()) {
                    return Err(format!(
                        "field `{}` watches `{}`, which is not in this form",
                        f.name,
                        trigger.watched()
                    ));
                }
            }
            let ft = field_types.get(&f.field_type).ok_or_else(|| {
                format!(
                    "field `{}` uses unregistered type `{}`",
                    f.name, f.field_type
                )
            })?;
            crate::field::check_field_keys(f, ft.as_ref())
                .map_err(|e| format!("field `{}` (`{}`): {e}", f.name, f.field_type))?;
            if let Some(format) = &f.format {
                format
                    .check()
                    .map_err(|e| format!("field `{}`: {e}", f.name))?;
            }
            let opts = ft
                .resolve_options(&f.options, field_types)
                .map_err(|e| format!("field `{}` (`{}`): {e}", f.name, f.field_type))?;
            let mut rules = ft.intrinsic_rules(&opts);
            rules.extend(f.rules.clone());
            let source = ft.option_source(&opts);
            fields.push(PreparedField {
                field_type: ft.clone(),
                opts,
                rules,
                source,
            });
        }
        check_dependencies(&config.fields)?;
        let persister: Arc<dyn Persister> = match &config.persist {
            Some(key) => persisters
                .get(key)
                .cloned()
                .ok_or_else(|| format!("form names unregistered persister `{key}`"))?,
            None => Arc::new(DefaultPersister::from_config(&config, field_types)),
        };
        // The framework's own listeners run first, so a contributed one can see
        // or override what they set.
        let mut resolved: Vec<Arc<dyn ModelListener>> = Vec::new();
        if config.timestamps {
            resolved.push(Arc::new(laterite_core::Timestamps));
        }
        if config.footprints {
            resolved.push(Arc::new(laterite_core::Footprints));
        }
        if !config.slugs.is_empty() {
            let rules = config
                .slugs
                .iter()
                .map(|(column, sources)| laterite_core::listeners::SlugRule {
                    column: column.clone(),
                    sources: sources.list(),
                })
                .collect();
            resolved.push(Arc::new(laterite_core::Sluggable::new(
                config.id_field.clone(),
                rules,
            )));
        }
        resolved.extend(
            listeners
                .iter()
                .filter(|reg| reg.matches(&config.entity))
                .map(|reg| reg.listener.clone()),
        );
        let listeners = resolved;
        Ok(Self {
            config,
            fields,
            persister,
            listeners,
            stay: false,
            update_permission: None,
            editable: true,
            rules: Arc::new(Default::default()),
        })
    }
}

/// The submission as a record, carrying only the fields the descriptor declares.
/// A request cannot introduce an attribute this way, so the persister may widen
/// its write to whatever the record holds; only registered listeners add keys.
///
/// Each value passes through its field type, which decides what gets stored and
/// may leave the attribute out entirely (a blank password on an edit).
fn declared_record(
    form: &PreparedForm,
    data: &HashMap<String, String>,
    mode: Mode,
) -> Result<Record, String> {
    let mut rec = Record::new(&form.config.entity);
    for (field, prepared) in form.config.fields.iter().zip(&form.fields) {
        if field.related.is_some() {
            continue;
        }
        let submitted = crate::field::SubmittedField::new(&field.name, data);
        if let Some(value) = prepared
            .field_type
            .to_attr(&submitted, &prepared.opts, mode)?
        {
            rec.set(field.name.clone(), value);
        }
    }
    Ok(rec)
}

/// The submission with every field present under its own name.
///
/// A field of several controls submits keys of its own and none under its name.
/// Its type gathers them into one value, so the rules see the field and a
/// refused save is re-rendered from what the operator entered. A key already
/// present is left as submitted.
pub(crate) fn gathered(
    form: &PreparedForm,
    mut data: HashMap<String, String>,
) -> HashMap<String, String> {
    for (field, prepared) in form.config.fields.iter().zip(&form.fields) {
        if data.contains_key(&field.name) {
            continue;
        }
        let value = prepared.field_type.submitted(
            &crate::field::SubmittedField::new(&field.name, &data),
            &prepared.opts,
        );
        if let Some(value) = value {
            data.insert(field.name.clone(), value);
        }
    }
    data
}

/// The merged validation rules for every field, in order.
fn merged_field_rules(form: &PreparedForm) -> Vec<FieldRules> {
    form.config
        .fields
        .iter()
        .zip(&form.fields)
        // The rules carry the label's source string; the validation message re-wraps
        // it as a nested Text, so it localizes at render like any other label.
        .map(|(f, pf)| FieldRules::new(f.name.clone(), f.label.source(), pf.rules.clone()))
        .collect()
}

/// The write a submit and a [`FormHandle`] share: `data` (already gathered)
/// validated against the merged rules, shaped into the declared record and saved
/// through the listeners and the persister. `id` names the row an edit updates;
/// `None` creates. Returns the record as saved, its id set on a create.
pub(crate) async fn write(
    db: &Db,
    cache: Option<&laterite_core::Cache>,
    form: &PreparedForm,
    data: &HashMap<String, String>,
    id: Option<&str>,
    actor: &Actor,
) -> Result<Record, SaveError> {
    let (mut rec, op) = prepare(db, form, data, id).await?;
    // The related records are written from the same stored shapes.
    let mut stored = data.clone();
    for field in &form.config.fields {
        if let (Some(format), Some(value)) = (&field.format, data.get(&field.name)) {
            if let Ok(shape) = format.normalize(value) {
                stored.insert(field.name.clone(), shape);
            }
        }
    }
    let graph = Graph {
        form,
        data: &stored,
    };
    persist::save(
        persist::SaveRequest {
            db,
            cache,
            listeners: &form.listeners,
            persister: form.persister.as_ref(),
            actor,
            op,
            id,
            then: Some(&graph),
        },
        &mut rec,
    )
    .await?;
    Ok(rec)
}

/// What a form writes beside its own record, in the same transaction: each
/// related one-to-one record, and the media links its fields hold.
struct Graph<'a> {
    form: &'a PreparedForm,
    data: &'a HashMap<String, String>,
}

impl Graph<'_> {
    /// The related record `name` as this submission fills it: each of its
    /// fields through its type. `None` when the submission carries none.
    fn related_record(
        &self,
        name: &str,
        related: &RelatedRecord,
        mode: Mode,
    ) -> Result<Option<Record>, SaveError> {
        let mut rec = Record::new(&related.entity);
        for (field, prepared) in self.form.config.fields.iter().zip(&self.form.fields) {
            if field.related.as_deref() != Some(name) {
                continue;
            }
            let submitted = crate::field::SubmittedField::new(&field.name, self.data);
            if let Some(value) = prepared
                .field_type
                .to_attr(&submitted, &prepared.opts, mode)
                .map_err(SaveError::Failed)?
            {
                rec.set(field.name.clone(), value);
            }
        }
        Ok((!rec.is_empty()).then_some(rec))
    }
}

/// Inserts `rec` into its table, or updates the row `id` with it, on the
/// save's transaction. Returns the row's id.
async fn write_related(
    cx: &mut laterite_core::SaveCx<'_>,
    related: &RelatedRecord,
    rec: &Record,
    id: Option<i64>,
) -> Result<i64, SaveError> {
    let backend = cx.backend();
    let fail = |e: sqlx::Error| SaveError::Failed(format!("{}: {e}", related.entity));
    let mut stamped;
    let rec = if related.timestamps {
        let now = chrono::Utc::now();
        stamped = rec.clone();
        stamped.set(laterite_core::listeners::UPDATED_AT, now);
        if id.is_none() {
            stamped.set(laterite_core::listeners::CREATED_AT, now);
        }
        &stamped
    } else {
        rec
    };
    match id {
        Some(id) => {
            let (sql, values) = {
                let mut update = Query::update();
                update.table(Alias::new(&related.entity));
                for (key, value) in rec.iter() {
                    update.value(Alias::new(key), persist::bind_value(Some(value)));
                }
                update.and_where(Expr::col(Alias::new(related.id_field())).eq(id));
                to_sql(backend, update)
            };
            bind_values(sqlx::query(&sql), values)
                .execute(cx.conn())
                .await
                .map_err(fail)?;
            Ok(id)
        }
        None => {
            let stmt = {
                let keys: Vec<&str> = rec.iter().map(|(k, _)| k).collect();
                Query::insert()
                    .into_table(Alias::new(&related.entity))
                    .columns(keys.iter().map(|k| Alias::new(*k)))
                    .values_panic(keys.iter().map(|k| persist::bind_value(rec.get(k))))
                    .to_owned()
            };
            laterite_core::query::insert_returning_id_on(
                backend,
                cx.conn(),
                stmt,
                Alias::new(related.id_field()),
            )
            .await
            .map_err(fail)
        }
    }
}

/// The id in `column` of `table`'s row whose `key` is `value`, read on the
/// save's transaction.
async fn lookup_id(
    cx: &mut laterite_core::SaveCx<'_>,
    table: &str,
    column: &str,
    key: &str,
    value: i64,
) -> Result<Option<i64>, SaveError> {
    let backend = cx.backend();
    let (sql, values) = {
        let mut select = Query::select();
        select
            .expr_as(
                Expr::col(Alias::new(column)).cast_as(Alias::new(text_cast(backend))),
                Alias::new("lat_id"),
            )
            .from(Alias::new(table))
            .and_where(Expr::col(Alias::new(key)).eq(value))
            .limit(1);
        to_sql(backend, select)
    };
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(cx.conn())
        .await
        .map_err(|e| SaveError::Failed(e.to_string()))?;
    Ok(row
        .and_then(|r| r.get_text_opt("lat_id").ok().flatten())
        .and_then(|v| v.parse().ok()))
}

#[laterite_core::strata::async_trait]
impl persist::InTx for Graph<'_> {
    /// Belongs-to records first, so this record can carry their ids.
    async fn before(
        &self,
        cx: &mut laterite_core::SaveCx<'_>,
        rec: &mut Record,
        op: Op,
    ) -> Result<(), SaveError> {
        let mode = if op == Op::Create {
            Mode::Create
        } else {
            Mode::Update
        };
        for (name, related) in &self.form.config.related {
            let Some(column) = &related.belongs_to else {
                continue;
            };
            let Some(child) = self.related_record(name, related, mode)? else {
                continue;
            };
            let current = match rec.id() {
                Some(id) => {
                    lookup_id(
                        cx,
                        &self.form.config.entity,
                        column,
                        &self.form.config.id_field,
                        id,
                    )
                    .await?
                }
                None => None,
            };
            let empty = child
                .iter()
                .all(|(_, value)| value.to_text().is_none_or(|text| text.trim().is_empty()));
            if related.optional && empty {
                if current.is_some() {
                    rec.set(column.clone(), laterite_core::AttrValue::Null);
                }
                continue;
            }
            let id = write_related(cx, related, &child, current).await?;
            rec.set(column.clone(), id);
        }
        Ok(())
    }

    /// Has-one records once this record has its id, then the media links.
    async fn after(
        &self,
        cx: &mut laterite_core::SaveCx<'_>,
        rec: &Record,
        op: Op,
    ) -> Result<(), SaveError> {
        let Some(owner_id) = rec.id() else {
            return Ok(());
        };
        let mode = if op == Op::Create {
            Mode::Create
        } else {
            Mode::Update
        };
        for (name, related) in &self.form.config.related {
            let Some(foreign_key) = &related.foreign_key else {
                continue;
            };
            let Some(mut child) = self.related_record(name, related, mode)? else {
                continue;
            };
            let current = lookup_id(
                cx,
                &related.entity,
                related.id_field(),
                foreign_key,
                owner_id,
            )
            .await?;
            child.set(foreign_key.clone(), owner_id);
            write_related(cx, related, &child, current).await?;
        }
        let owner = owner_id.to_string();
        let backend = cx.backend();
        for (field, prepared) in self.form.config.fields.iter().zip(&self.form.fields) {
            let submitted = self.data.get(&field.name).map(String::as_str);
            let Some(ids) = prepared.field_type.linked_media(submitted, &prepared.opts) else {
                continue;
            };
            let link = laterite_media::Owner {
                owner_type: &self.form.config.entity,
                owner_id: &owner,
                field: &field.name,
            };
            laterite_media::link::set_on(cx.conn(), backend, link, &ids)
                .await
                .map_err(|e| SaveError::Failed(format!("media links: {e}")))?;
        }
        Ok(())
    }
}

/// The write up to the point of writing: the data validated and shaped into
/// the record, with the operation it would be. What a check runs, and what a
/// write runs first.
async fn prepare(
    db: &Db,
    form: &PreparedForm,
    data: &HashMap<String, String>,
    id: Option<&str>,
) -> Result<(Record, Op), SaveError> {
    if !form.config.idents_valid() {
        return Err(SaveError::Failed(format!(
            "form `{}` names an invalid entity or field identifier",
            form.config.entity
        )));
    }
    let (mode, op) = match id {
        None => (Mode::Create, Op::Create),
        Some(_) => (Mode::Update, Op::Update),
    };
    // A formatted field is stored in its one shape; a value in none of the
    // shapes it accepts is refused on the field.
    let mut formatted = ErrorBag::default();
    let mut normalized = data.clone();
    for field in &form.config.fields {
        let (Some(format), Some(value)) = (&field.format, data.get(&field.name)) else {
            continue;
        };
        match format.normalize(value) {
            Ok(stored) => {
                normalized.insert(field.name.clone(), stored);
            }
            Err(message) => formatted.add(&field.name, message),
        }
    }
    let data = &normalized;
    let fields = merged_field_rules(form);
    let mut bag = validate(
        db,
        &form.config.entity,
        &form.config.id_field,
        &fields,
        data,
        mode,
        id,
    )
    .await
    .map_err(|e| SaveError::Failed(e.to_string()))?;
    for (field, messages) in formatted.iter() {
        for message in messages {
            bag.add(field, message.clone());
        }
    }
    laterite_core::validation::validate_custom(
        db,
        &form.config.entity,
        &fields,
        data,
        mode,
        id,
        &form.rules,
        &mut bag,
    )
    .await;
    // A field shown to people who are not signed in takes no private file.
    for (field, prepared) in form.config.fields.iter().zip(&form.fields) {
        let submitted = data.get(&field.name).map(String::as_str);
        if let Some(name) =
            crate::medialib::field::private_pick(db, &prepared.opts, submitted).await
        {
            bag.add(&field.name, crate::medialib::field::private_refusal(&name));
        }
    }
    if !bag.is_empty() {
        return Err(SaveError::Invalid(bag));
    }
    let mut rec = declared_record(form, data, mode).map_err(|e| {
        tracing::error!(entity = %form.config.entity, error = %e, "preparing the write failed");
        SaveError::Failed(e)
    })?;
    if let Some(n) = id.and_then(|id| id.parse::<i64>().ok()) {
        rec.set_id(n);
    }
    Ok((rec, op))
}

/// The forms the resources mounted, keyed by each resource's path as its
/// descriptor declares it, relative to the admin root.
pub(crate) type Forms = Arc<HashMap<String, Arc<PreparedForm>>>;

/// A resource's form, written through from code.
///
/// Runs what the form's own submit runs, with no request in hand: the values
/// are validated against the form's rules, shaped by its field types and saved
/// through its listeners and persister, so an importer or a sync writes exactly
/// what an operator could have typed, audited the same way. A screen reaches one
/// through [`crate::routes::RouteCtx::form`].
#[derive(Clone)]
pub struct FormHandle {
    db: Db,
    cache: laterite_core::Cache,
    form: Arc<PreparedForm>,
}

impl FormHandle {
    pub(crate) fn new(db: Db, cache: laterite_core::Cache, form: Arc<PreparedForm>) -> Self {
        Self { db, cache, form }
    }

    /// The table the form writes.
    pub fn entity(&self) -> &str {
        &self.form.config.entity
    }

    /// The primary key column.
    pub fn id_field(&self) -> &str {
        &self.form.config.id_field
    }

    /// The fields the form declares, in order. `data` is keyed by their names.
    pub fn fields(&self) -> &[FormField] {
        &self.form.config.fields
    }

    /// Validates `data` as a submission and saves a new record, returning its id.
    ///
    /// `data` holds each field's value under the field's name, as the form's
    /// control submits it; a field left out is blank. `Invalid` carries the
    /// per-field messages a refused submit would show.
    pub async fn create(
        &self,
        data: HashMap<String, String>,
        actor: &Actor,
    ) -> Result<i64, SaveError> {
        let data = gathered(&self.form, data);
        let rec = write(&self.db, Some(&self.cache), &self.form, &data, None, actor).await?;
        rec.id()
            .ok_or_else(|| SaveError::Failed("the persister returned no id".to_string()))
    }

    /// The record `id` as the edit form shows it: each stored value back through
    /// its field type, keyed by field name. `None` when no record has that id.
    /// Lay a partial `data` over it before [`update`](Self::update) to change
    /// some fields and keep the rest.
    pub async fn load(&self, id: i64) -> Result<Option<HashMap<String, String>>, SaveError> {
        load_values(&self.db, &self.form, &id.to_string(), &[])
            .await
            .map_err(|e| SaveError::Failed(e.to_string()))
    }

    /// What [`create`](Self::create) or [`update`](Self::update) would do with
    /// `data`, with nothing written: the rules and the field conversions run,
    /// the listeners and the persister do not. `id` names the record an edit
    /// would change; `None` checks as a create.
    pub async fn check(
        &self,
        id: Option<i64>,
        data: HashMap<String, String>,
    ) -> Result<(), SaveError> {
        let data = gathered(&self.form, data);
        let id = id.map(|id| id.to_string());
        prepare(&self.db, &self.form, &data, id.as_deref())
            .await
            .map(|_| ())
    }

    /// Validates `data` as an edit of the record `id` and saves it. Every field
    /// is written, so a field left out of `data` is stored blank.
    pub async fn update(
        &self,
        id: i64,
        data: HashMap<String, String>,
        actor: &Actor,
    ) -> Result<(), SaveError> {
        let data = gathered(&self.form, data);
        write(
            &self.db,
            Some(&self.cache),
            &self.form,
            &data,
            Some(&id.to_string()),
            actor,
        )
        .await
        .map(|_| ())
    }
}

/// The longest value the rules accept, when they set one.
pub(crate) fn max_length(rules: &[Rule]) -> Option<usize> {
    rules
        .iter()
        .filter_map(|r| match r {
            Rule::MaxLength(n) => Some(*n),
            _ => None,
        })
        .min()
}

/// Whether any rule marks the field required.
fn has_required(rules: &[Rule]) -> bool {
    rules
        .iter()
        .any(|r| matches!(r, Rule::Required | Rule::RequiredOn(_)))
}

/// Renders an empty create form.
pub(crate) async fn new_form(
    state: &AdminState,
    form: &PreparedForm,
    shell: crate::Shell,
    pending: Vec<String>,
) -> Response {
    let seen = form.seen_by(|permission| shell.allows(permission));
    let form: &PreparedForm = &seen;
    let values = form.defaults(&shell);
    let loaded = form.loaded(state, &values, None, &shell).await;
    let form: &PreparedForm = &loaded;
    let mut page = build(
        state,
        form,
        &format!("{}/new", form.config.base_path),
        None,
        &values,
        &ErrorBag::default(),
        &shell,
    );
    page.pending = pending;
    render(page)
}

/// Where a save goes: the edit page when the operator asked to stay; the
/// form's `after_save` address for this kind of save, unless they asked to
/// close; else the form's done path or its list.
fn after_save(
    form: &PreparedForm,
    data: &HashMap<String, String>,
    id: Option<i64>,
    created: bool,
    base: &str,
) -> String {
    let then = data.get("then").map(String::as_str);
    let list = || {
        form.config
            .done_path
            .clone()
            .unwrap_or_else(|| form.config.base_path.clone())
    };
    let rules = &form.config.after_save;
    match (then, id) {
        (Some("stay"), Some(id)) => format!("{}/{}/edit", form.config.base_path, id),
        (Some("close"), _) => match &rules.close {
            Some(path) => AfterSave::address(path, base, id),
            None => list(),
        },
        _ => {
            let own = if created {
                &rules.create
            } else {
                &rules.update
            };
            match own {
                Some(path) => AfterSave::address(path, base, id),
                None => list(),
            }
        }
    }
}

/// Persists a new record, then redirects to the list. Re-renders with per-field
/// errors when validation fails.
pub(crate) async fn create(
    state: &AdminState,
    form: &PreparedForm,
    data: HashMap<String, String>,
    shell: crate::Shell,
    user: &laterite_auth::AuthenticatedUser,
    session: &crate::session::SessionHandle,
    headers: &axum::http::HeaderMap,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    let htmx = is_htmx(headers);
    let action = format!("{}/new", form.config.base_path);
    let seen = form.seen_by(|permission| shell.allows(permission));
    let form: &PreparedForm = &seen;
    let loaded = form.loaded(state, &data, None, &shell).await;
    let form: &PreparedForm = &loaded;
    let mut data = gathered(form, data);
    // A read-only field starts from its default, whatever was submitted.
    let defaults = form.defaults(&shell);
    for field in form.config.fields.iter().filter(|f| f.read_only) {
        match defaults.get(&field.name) {
            Some(value) => data.insert(field.name.clone(), value.clone()),
            None => data.remove(&field.name),
        };
    }
    let then = data.remove("then");
    let actor = Actor::from(user);
    let written = form.written(false);
    match write(&state.db, Some(&state.cache), &written, &data, None, &actor).await {
        // The audit entry is written by the framework's audit listener, which
        // runs on every entity after the commit.
        Ok(rec) => {
            let said = form.config.after_save.created.clone();
            session.push_flash(
                crate::session::FlashLevel::Success,
                said.unwrap_or_else(|| t!("Saved.")),
            );
            let mut choice = HashMap::new();
            if let Some(then) = then {
                choice.insert("then".to_string(), then);
            }
            saved_response(
                htmx,
                &after_save(form, &choice, rec.id(), true, &shell.base),
            )
        }
        // A refused submission re-renders the form with per-field errors as 422,
        // the cross-surface "validation failure" status.
        Err(SaveError::Invalid(bag)) => {
            invalid_response(htmx, state, form, &action, None, &data, &bag, &shell)
        }
        Err(SaveError::Failed(msg)) => {
            tracing::error!(error = %msg, "admin create failed");
            // The same shape as a refusal, so an HTMX submit swaps the form alone.
            invalid_response(
                htmx,
                state,
                form,
                &action,
                Some(failed_message(&msg)),
                &data,
                &ErrorBag::default(),
                &shell.clone().with_error_detail(&msg),
            )
        }
    }
}

/// Renders a form populated with an existing record.
pub(crate) async fn edit_form(
    state: &AdminState,
    form: &PreparedForm,
    id: String,
    shell: crate::Shell,
    sections: Vec<RelationSection>,
    permissions: &laterite_auth::PermissionSet,
    delete: Option<&DeleteOffer>,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    let seen = form.seen_by(|permission| shell.allows(permission));
    let form: &PreparedForm = &seen;
    let values =
        match load_values(&state.db, form, &id, state.scopes_for(&form.config.entity)).await {
            Ok(Some(values)) => values,
            Ok(None) => return not_found(),
            Err(_) => return render_error(),
        };
    let loaded = form.loaded(state, &values, Some(&id), &shell).await;
    let form: &PreparedForm = &loaded;

    let mut page = build(
        state,
        form,
        &format!("{}/{}/edit", form.config.base_path, id),
        None,
        &values,
        &ErrorBag::default(),
        &shell,
    );
    page.sections = sections;
    page.panels = panel_views(&form.config.panels, &id, &shell);
    page.linked = linked_view(state, &form.config, &id, permissions, &shell).await;
    if let Some(offer) = delete {
        page.delete = offer
            .view(state, &form.config, &id, permissions, &shell)
            .await;
    }
    render(page)
}

impl FormConfig {
    /// Moves the fields under `tabs` and `secondary_tabs` in among the form's
    /// own, each marked with its part of the form and its tab, so everything
    /// that reads or writes the form sees one list. A field among the form's
    /// own that names a `tab` joins the primary tabs. Safe to call twice.
    pub fn absorb_tabs(&mut self) {
        for field in &mut self.fields {
            if field.tab.is_some() && field.area == FormArea::Outside {
                field.area = FormArea::Primary;
            }
        }
        for (set, area) in [
            (&mut self.tabs, FormArea::Primary),
            (&mut self.secondary_tabs, FormArea::Secondary),
        ] {
            let Some(set) = set else { continue };
            let default = set.default_tab.clone().unwrap_or_else(|| Text::new("Misc"));
            for mut field in std::mem::take(&mut set.fields) {
                field.area = area;
                field.tab.get_or_insert_with(|| default.clone());
                self.fields.push(field);
            }
        }
        // A field on the primary tabs with no set declared still needs one.
        if self.tabs.is_none() && self.fields.iter().any(|f| f.area == FormArea::Primary) {
            self.tabs = Some(FormTabs::default());
        }
    }

    /// The form as `page` shows it: the fields on that page, each column once.
    pub fn for_context(&self, page: FormContext) -> FormConfig {
        let mut form = self.clone();
        form.absorb_tabs();
        let mut seen = std::collections::HashSet::new();
        form.fields
            .retain(|f| f.on(page) && seen.insert(f.name.clone()));
        form
    }

    /// The form with every field whatever its page, each column once: what a
    /// caller with no page of its own (an import, an ability) writes through.
    pub fn every_context(&self) -> FormConfig {
        let mut form = self.clone();
        form.absorb_tabs();
        let mut seen = std::collections::HashSet::new();
        form.fields.retain(|f| seen.insert(f.name.clone()));
        form
    }
}

/// A record's preview page: its form's values to read, with nothing to save,
/// at `{base_path}/{id}/preview`. The list gains a Preview link on each row.
///
/// ```yaml
/// preview:
///   title: Order
///   assets: [chart.css]
///   panels:
///     - { title: Payments, src: "acme/orders/{id}/payments" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PreviewConfig {
    /// The page's title; the form's when left out.
    #[serde(default)]
    pub title: Option<Text>,
    /// Asset-registry keys the panels need on the page.
    #[serde(default)]
    pub assets: Vec<String>,
    /// Parts a module draws under the values.
    #[serde(default)]
    pub panels: Vec<PreviewPanel>,
    /// The fields the preview shows, in the form's own shape. Left out, it
    /// shows the form's fields that are on the preview page.
    #[serde(
        default,
        deserialize_with = "crate::keyed::deserialize",
        serialize_with = "crate::keyed::serialize"
    )]
    pub fields: Vec<FormField>,
    /// Whether the record's related lists show under the values. Default
    /// `true`.
    #[serde(default = "yes")]
    pub relations: bool,
}

fn yes() -> bool {
    true
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            title: None,
            assets: Vec::new(),
            panels: Vec::new(),
            fields: Vec::new(),
            relations: true,
        }
    }
}

/// One part of a preview a module draws: the page fetches `src` once it is
/// open and shows what comes back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewPanel {
    /// The heading over the panel, when it has one.
    #[serde(default)]
    pub title: Option<Text>,
    /// A path under the admin root answering an HTML fragment, `{id}`
    /// replaced by the record's id.
    pub src: String,
    /// A permission the operator must hold to see the panel.
    #[serde(default)]
    pub permission: Option<String>,
}

/// A preview panel as the page draws it.
pub(crate) struct PanelView {
    title: Option<String>,
    src: String,
}

/// Where a save goes, and what it says, when the defaults do not suit: Save
/// returns to the list and says "Saved.".
///
/// ```yaml
/// form:
///   after_save:
///     create: "acme/posts/{id}/edit"
///     close: acme/posts
///     created: Post created.
/// ```
///
/// Each address is a path under the admin root, `{id}` replaced by the
/// record's id. A page whose Save goes somewhere other than the list offers
/// "Save and close" beside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct AfterSave {
    /// Where Save goes after a new record is created.
    pub create: Option<String>,
    /// Where Save goes after an existing record is updated.
    pub update: Option<String>,
    /// Where "Save and close" goes. The list when unset.
    pub close: Option<String>,
    /// What a create says in place of "Saved.".
    pub created: Option<Text>,
    /// What an update says in place of "Saved.".
    pub updated: Option<Text>,
}

impl AfterSave {
    /// Sends Save to `path` after a create.
    pub fn create(mut self, path: impl Into<String>) -> Self {
        self.create = Some(path.into());
        self
    }

    /// Sends Save to `path` after an update.
    pub fn update(mut self, path: impl Into<String>) -> Self {
        self.update = Some(path.into());
        self
    }

    /// Sends "Save and close" to `path`.
    pub fn close(mut self, path: impl Into<String>) -> Self {
        self.close = Some(path.into());
        self
    }

    /// Says `text` after a create.
    pub fn created(mut self, text: impl Into<Text>) -> Self {
        self.created = Some(text.into());
        self
    }

    /// Says `text` after an update.
    pub fn updated(mut self, text: impl Into<Text>) -> Self {
        self.updated = Some(text.into());
        self
    }

    /// Refuses an address that leaves the admin: one with a scheme, or one a
    /// browser reads as another host.
    fn check(&self) -> Result<(), String> {
        for path in [&self.create, &self.update, &self.close]
            .into_iter()
            .flatten()
        {
            if path.contains("://") || path.starts_with("//") || path.contains('\\') {
                return Err(format!(
                    "`after_save` goes to `{path}`, which is not a path under the admin"
                ));
            }
        }
        Ok(())
    }

    /// `path` under the admin root at `base`, for record `id`.
    fn address(path: &str, base: &str, id: Option<i64>) -> String {
        let id = id.map(|id| id.to_string()).unwrap_or_default();
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.replace("{id}", &id).trim_start_matches('/')
        )
    }
}

/// The panels the operator may see, each addressed for record `id`.
fn panel_views(panels: &[PreviewPanel], id: &str, shell: &crate::Shell) -> Vec<PanelView> {
    let base = shell.base.trim_end_matches('/');
    panels
        .iter()
        .filter(|p| p.permission.as_deref().is_none_or(|g| shell.allows(g)))
        .map(|p| PanelView {
            title: p.title.as_ref().map(|t| shell.tt(t)),
            src: format!(
                "{base}/{}",
                p.src.replace("{id}", id).trim_start_matches('/')
            ),
        })
        .collect()
}

/// The preview page for record `id`.
pub(crate) async fn preview_page(
    state: &AdminState,
    form: &PreparedForm,
    preview: &PreviewConfig,
    id: String,
    shell: crate::Shell,
    sections: Vec<RelationSection>,
    permissions: &laterite_auth::PermissionSet,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    let seen = form.seen_by(|permission| shell.allows(permission));
    let form: &PreparedForm = &seen;
    let values =
        match load_values(&state.db, form, &id, state.scopes_for(&form.config.entity)).await {
            Ok(Some(values)) => values,
            Ok(None) => return not_found(),
            Err(_) => return render_error(),
        };
    let loaded = form.loaded(state, &values, Some(&id), &shell).await;
    let form: &PreparedForm = &loaded;
    let edit = format!("{}/{}/edit", form.config.base_path, id);
    let mut page = build(
        state,
        form,
        &edit,
        None,
        &values,
        &ErrorBag::default(),
        &shell,
    );
    if let Some(title) = &preview.title {
        page.title = shell.tt(title);
    }
    if !preview.assets.is_empty() {
        let keys: Vec<&str> = preview.assets.iter().map(String::as_str).collect();
        let extra = crate::page_assets(&keys, &shell.base, &state.assets, &state.asset_urls);
        page.shell.assets.extend(extra);
    }
    page.read_only = true;
    page.preview = true;
    page.actions = Vec::new();
    page.edit_href = form.editable.then_some(edit);
    page.panels = panel_views(&preview.panels, &id, &shell);
    page.sections = sections;
    page.linked = linked_view(state, &form.config, &id, permissions, &shell).await;
    render(page)
}

/// The Delete an edit page offers: what the resource's list allows on a row.
#[derive(Clone)]
pub(crate) struct DeleteOffer {
    /// Where the list's Delete posts.
    pub(crate) action: String,
    /// The grant a delete asks for beside the resource's.
    pub(crate) permission: Option<String>,
    /// The list's `protected` column.
    pub(crate) protected: Option<String>,
    /// Delete moves the record to the trash.
    pub(crate) soft: bool,
    /// The relations that go with the record, by title.
    pub(crate) with: Vec<Text>,
}

impl DeleteOffer {
    /// The button for record `id`: none without the grant, or on a row the
    /// list protects.
    async fn view(
        &self,
        state: &AdminState,
        form: &FormConfig,
        id: &str,
        permissions: &laterite_auth::PermissionSet,
        shell: &crate::Shell,
    ) -> Option<DeleteView> {
        if self
            .permission
            .as_deref()
            .is_some_and(|p| !permissions.allows(p))
        {
            return None;
        }
        if let Some(column) = &self.protected {
            let backend = state.db.backend;
            let (sql, values) = {
                let mut select = Query::select();
                select
                    .expr_as(
                        Expr::col(Alias::new(column)).cast_as(Alias::new(text_cast(backend))),
                        Alias::new("lat_protected"),
                    )
                    .from(Alias::new(&form.entity))
                    .and_where(
                        Expr::expr(
                            Expr::col(Alias::new(&form.id_field))
                                .cast_as(Alias::new(text_cast(backend))),
                        )
                        .eq(id),
                    );
                to_sql(backend, select)
            };
            // A row that cannot be read is not offered for deleting.
            let row = bind_values(sqlx::query(&sql), values)
                .fetch_optional(&state.db.pool)
                .await
                .ok()??;
            let marked = row.get_text_opt("lat_protected").ok().flatten();
            if marked.is_some_and(|text| crate::list::truthy(&text)) {
                return None;
            }
        }
        let confirm = if self.soft {
            crate::list::confirm(
                shell,
                "Move this record to the trash?",
                "These go to the trash with it: {names}.",
                &self.with,
            )
        } else {
            crate::list::confirm(
                shell,
                "Delete this record? This cannot be undone.",
                "These are deleted with it: {names}.",
                &self.with,
            )
        };
        Some(DeleteView {
            action: self.action.clone(),
            id: id.to_string(),
            confirm,
        })
    }
}

/// The edit page's Delete button.
struct DeleteView {
    action: String,
    id: String,
    confirm: String,
}

/// The stored record `id` as control values: each column back through its
/// field type, since what a column holds and what its control shows are not
/// always the same string. `None` when no record has that id.
/// `GET {base}/{id}/reveal/{field}`: the stored value of a sensitive field,
/// opened, as plain text for the field's Reveal button. Recorded in the audit
/// log as `<entity>.reveal` with the field as the detail.
pub(crate) async fn reveal(
    state: &AdminState,
    form: &PreparedForm,
    id: &str,
    field: &str,
    user: &laterite_auth::AuthenticatedUser,
) -> Response {
    if !form.config.fields.iter().any(|f| {
        f.name == field
            && f.field_type == "sensitive"
            && !crate::field::SensitiveField::is_file(&f.options)
    }) {
        return crate::AdminError::NotFound.into_response();
    }
    // The column as stored, not as the control shows it (`load_values` masks it).
    let (sql, values) = {
        let cast = text_cast(state.db.backend);
        let mut select = Query::select();
        select
            .expr_as(
                Expr::col(Alias::new(field)).cast_as(Alias::new(cast)),
                Alias::new("sealed"),
            )
            .from(Alias::new(&form.config.entity))
            .and_where(
                Expr::col(Alias::new(&form.config.id_field))
                    .cast_as(Alias::new(cast))
                    .eq(id.to_string()),
            );
        laterite_core::scope::apply(
            state.scopes_for(&form.config.entity),
            &laterite_core::ScopeCx::new(&form.config.entity, state.db.backend),
            &mut select,
            &[],
        );
        to_sql(state.db.backend, select)
    };
    let sealed = match bind_values(sqlx::query(&sql), values)
        .fetch_optional(&state.db.pool)
        .await
    {
        Ok(Some(row)) => row
            .get_text_opt("sealed")
            .unwrap_or_default()
            .unwrap_or_default(),
        Ok(None) => return crate::AdminError::NotFound.into_response(),
        Err(e) => return crate::AdminError::Internal(anyhow::anyhow!(e)).into_response(),
    };
    let Some(plain) = crate::field::SensitiveField::open(&state.crypt, field, &sealed) else {
        return crate::AdminError::Internal(anyhow::anyhow!(
            "the value does not open under the application key"
        ))
        .into_response();
    };
    crate::audit::record(
        state,
        user,
        &format!("{}.reveal", form.config.entity),
        Some(&form.config.entity),
        Some(id),
        None,
        Some(field),
    )
    .await;
    (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            ),
            (axum::http::header::CACHE_CONTROL, "private, no-store"),
        ],
        plain,
    )
        .into_response()
}

pub(crate) async fn load_values(
    db: &Db,
    form: &PreparedForm,
    id: &str,
    hooks: &[Arc<dyn laterite_core::QueryHook>],
) -> Result<Option<HashMap<String, String>>, sqlx::Error> {
    // Scope the sea-query builder so it is dropped before the await below: its
    // identifiers are reference-counted (not `Send`), and a live builder across
    // the await would make the caller's future non-`Send`.
    let (sql, values) = {
        let cast = text_cast(db.backend);
        let mut select = Query::select();
        for (field, _) in form
            .config
            .fields
            .iter()
            .zip(&form.fields)
            .filter(|(f, pf)| !f.transient && f.related.is_none() && pf.field_type.stores())
        {
            select.expr_as(
                Expr::col(Alias::new(&field.name)).cast_as(Alias::new(cast)),
                Alias::new(&field.name),
            );
        }
        // A belongs-to record is found through this row's own column.
        for (name, related) in &form.config.related {
            if let Some(column) = &related.belongs_to {
                select.expr_as(
                    Expr::col(Alias::new(column)).cast_as(Alias::new(cast)),
                    Alias::new(format!("lat_rel_{name}")),
                );
            }
        }
        select.from(Alias::new(&form.config.entity)).and_where(
            Expr::col(Alias::new(&form.config.id_field))
                .cast_as(Alias::new(cast))
                .eq(id.to_string()),
        );
        laterite_core::scope::apply(
            hooks,
            &laterite_core::ScopeCx::new(&form.config.entity, db.backend),
            &mut select,
            &[],
        );
        to_sql(db.backend, select)
    };
    let Some(row) = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?
    else {
        return Ok(None);
    };
    // Each related record's stored values, by field name.
    let mut related_values: HashMap<String, Option<String>> = HashMap::new();
    for (name, related) in &form.config.related {
        let (key, value) = match (&related.foreign_key, &related.belongs_to) {
            (Some(fk), _) => (fk.clone(), Some(id.to_string())),
            (None, Some(_)) => (
                related.id_field().to_string(),
                row.get_text_opt(&format!("lat_rel_{name}")).ok().flatten(),
            ),
            (None, None) => continue,
        };
        let fields: Vec<&FormField> = form
            .config
            .fields
            .iter()
            .filter(|f| f.related.as_deref() == Some(name.as_str()))
            .collect();
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            continue;
        };
        let (sql, values) = {
            let cast = text_cast(db.backend);
            let mut select = Query::select();
            for f in &fields {
                select.expr_as(
                    Expr::col(Alias::new(&f.name)).cast_as(Alias::new(cast)),
                    Alias::new(&f.name),
                );
            }
            select
                .from(Alias::new(&related.entity))
                .and_where(
                    Expr::col(Alias::new(&key))
                        .cast_as(Alias::new(cast))
                        .eq(value),
                )
                .limit(1);
            to_sql(db.backend, select)
        };
        if let Some(rel_row) = bind_values(sqlx::query(&sql), values)
            .fetch_optional(&db.pool)
            .await?
        {
            for f in fields {
                related_values.insert(f.name.clone(), rel_row.get_text_opt(&f.name).ok().flatten());
            }
        }
    }
    Ok(Some(
        form.config
            .fields
            .iter()
            .zip(&form.fields)
            .map(|(f, prepared)| {
                let stored = if f.related.is_some() {
                    related_values.get(&f.name).cloned().flatten()
                } else if f.transient || !prepared.field_type.stores() {
                    None
                } else {
                    row.get_text_opt(f.name.as_str()).ok().flatten()
                };
                let value =
                    prepared
                        .field_type
                        .to_control_for(&f.name, stored.as_deref(), &prepared.opts);
                (f.name.clone(), value)
            })
            .collect(),
    ))
}

/// Persists an edited record, then redirects to the list. Re-renders with
/// per-field errors when validation fails.
// The generic handlers thread the request's pieces explicitly rather than through
// a context struct; the list is long but each argument is named at the call site.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn update(
    state: &AdminState,
    form: &PreparedForm,
    id: String,
    data: HashMap<String, String>,
    shell: crate::Shell,
    user: &laterite_auth::AuthenticatedUser,
    session: &crate::session::SessionHandle,
    headers: &axum::http::HeaderMap,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    let htmx = is_htmx(headers);
    let action = format!("{}/{}/edit", form.config.base_path, id);
    let seen = form.seen_by(|permission| shell.allows(permission));
    let form: &PreparedForm = &seen;
    let loaded = form.loaded(state, &data, Some(&id), &shell).await;
    let form: &PreparedForm = &loaded;
    let mut data = gathered(form, data);
    let then = data.remove("then");
    let actor = Actor::from(user);
    let written = form.written(true);
    match write(
        &state.db,
        Some(&state.cache),
        &written,
        &data,
        Some(&id),
        &actor,
    )
    .await
    {
        Ok(_) => {
            let said = form.config.after_save.updated.clone();
            session.push_flash(
                crate::session::FlashLevel::Success,
                said.unwrap_or_else(|| t!("Saved.")),
            );
            let mut choice = HashMap::new();
            if let Some(then) = then {
                choice.insert("then".to_string(), then);
            }
            saved_response(
                htmx,
                &after_save(form, &choice, id.parse().ok(), false, &shell.base),
            )
        }
        // A refused submission re-renders the form with per-field errors as 422,
        // the cross-surface "validation failure" status.
        Err(SaveError::Invalid(bag)) => {
            invalid_response(htmx, state, form, &action, None, &data, &bag, &shell)
        }
        Err(SaveError::Failed(msg)) => {
            tracing::error!(error = %msg, "admin update failed");
            // The same shape as a refusal, so an HTMX submit swaps the form alone.
            invalid_response(
                htmx,
                state,
                form,
                &action,
                Some(failed_message(&msg)),
                &data,
                &ErrorBag::default(),
                &shell.clone().with_error_detail(&msg),
            )
        }
    }
}

/// The 422 response for a failed save. An HTMX submit gets the form alone, which
/// replaces the form in place; anything else gets the whole page, so the admin
/// still works with scripting off.
#[allow(clippy::too_many_arguments)]
/// The fields named in the submission's `_fields`, drawn again holding what
/// was submitted: what a field following another is replaced with when the
/// other changes. Each comes back in its own wrapper; a name the form lacks
/// is passed over.
pub(crate) async fn refresh(
    state: &AdminState,
    form: &PreparedForm,
    id: Option<&str>,
    mut data: HashMap<String, String>,
    shell: crate::Shell,
) -> Response {
    let wanted: Vec<String> = data
        .remove("_fields")
        .unwrap_or_default()
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    let seen = form.seen_by(|permission| shell.allows(permission));
    let form: &PreparedForm = &seen;
    let loaded = form.loaded(state, &data, id, &shell).await;
    let form: &PreparedForm = &loaded;
    let data = gathered(form, data);
    let action = match id {
        Some(id) => format!("{}/{}/edit", form.config.base_path, id),
        None => format!("{}/new", form.config.base_path),
    };
    let page = build(
        state,
        form,
        &action,
        None,
        &data,
        &ErrorBag::default(),
        &shell,
    );
    render(FieldsFragment {
        fields: page
            .fields
            .into_iter()
            .filter(|view| view.chrome.is_none() && wanted.contains(&view.id))
            .collect(),
    })
}

/// A form page posting to `action`, holding `values`: the shape another screen
/// borrows when it edits one thing a record owns, such as its tree parent.
pub(crate) fn page(
    state: &AdminState,
    form: &PreparedForm,
    action: &str,
    values: &HashMap<String, String>,
    shell: &crate::Shell,
) -> Response {
    render(build(
        state,
        form,
        action,
        None,
        values,
        &ErrorBag::default(),
        shell,
    ))
}

/// The 422 for a submission the list's own checks refused, shaped like a
/// failed save: the form again with the messages against their fields.
#[allow(clippy::too_many_arguments)]
pub(crate) fn refused(
    htmx: bool,
    state: &AdminState,
    form: &PreparedForm,
    action: &str,
    error: Option<Text>,
    values: &HashMap<String, String>,
    bag: &ErrorBag,
    shell: &crate::Shell,
) -> Response {
    invalid_response(htmx, state, form, action, error, values, bag, shell)
}

#[allow(clippy::too_many_arguments)]
fn invalid_response(
    htmx: bool,
    state: &AdminState,
    form: &PreparedForm,
    action: &str,
    error: Option<Text>,
    values: &HashMap<String, String>,
    bag: &ErrorBag,
    shell: &crate::Shell,
) -> Response {
    let page = build(state, form, action, error, values, bag, shell);
    let body = if htmx {
        render(FormFragment {
            shell: page.shell,
            action: page.action,
            cancel_path: page.cancel_path,
            error: page.error,
            fields: page.fields,
            stay: page.stay,
            close: page.close,
            actions: page.actions,
            read_only: page.read_only,
            preview: false,
            enter_off: page.enter_off,
            refused: page.refused,
            leave_off: page.leave_off,
            focus: page.focus,
        })
    } else {
        render(page)
    };
    (StatusCode::UNPROCESSABLE_ENTITY, body).into_response()
}

/// What a form says when the database refused the save: that a value is taken
/// when a unique column refused it. The database's own words travel beside it,
/// for whoever may read them (see `Shell::with_error_detail`).
fn failed_message(error: &str) -> Text {
    let taken = [
        "UNIQUE constraint failed",
        "duplicate key value",
        "Duplicate entry",
    ]
    .iter()
    .any(|sign| error.contains(sign));
    if taken {
        t!("Could not save: a value here is already used by another record.")
    } else {
        t!("Could not save. Check the values and try again.")
    }
}

/// Where to send the browser after a successful save. HTMX will not follow a
/// 303 usefully (it would swap the redirected page into the form), so it gets
/// the header it understands instead.
pub(crate) fn saved_response(htmx: bool, to: &str) -> Response {
    if htmx {
        ([("HX-Redirect", to)], StatusCode::NO_CONTENT).into_response()
    } else {
        Redirect::to(to).into_response()
    }
}

/// Whether this request came from HTMX rather than a plain form post.
pub(crate) fn is_htmx(headers: &axum::http::HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

fn build(
    state: &AdminState,
    form: &PreparedForm,
    action: &str,
    error: Option<Text>,
    values: &HashMap<String, String>,
    bag: &ErrorBag,
    shell: &crate::Shell,
) -> FormTemplate {
    let fields = form
        .config
        .fields
        .iter()
        .zip(&form.fields)
        .map(|(f, pf)| {
            let value = FieldValue::Text(values.get(&f.name).cloned().unwrap_or_default());
            let required = has_required(&pf.rules);
            // Localize the label once: the field type sees it (aria/inline labels)
            // and the framework chrome renders it.
            let label = shell.tt(&f.label);
            let cx = FieldCx {
                name: &f.name,
                id: &f.name,
                label: &label,
                value: &value,
                required,
                max_length: max_length(&pf.rules),
                opts: &pf.opts,
                base: &shell.base,
                i18n: shell.i18n(),
                icons: crate::icons::Icons::new(&shell.icon_set, &shell.sprite_url),
            };
            // The type is registered (prepare validated it); the lookup here is
            // only to render.
            let control = if f.hidden {
                format!(
                    r#"<input type="hidden" name="{}" value="{}">"#,
                    escape_attr(&f.name),
                    escape_attr(values.get(&f.name).map(String::as_str).unwrap_or_default())
                )
            } else {
                match state.field_types.get(&f.field_type) {
                    Some(ft) => {
                        let scope = OverrideScope {
                            surface: Surface::Field,
                            view_key: &f.field_type,
                            resource: Some(&form.config.base_path),
                            field: Some(&f.name),
                        };
                        render_field(ft.as_ref(), state.overrides.as_ref(), &scope, &cx)
                            .into_string()
                    }
                    None => String::new(),
                }
            };
            let control = if f.hidden {
                control
            } else {
                f.with_help_above(control, shell)
            };
            // A locked field shows its value and takes no input. A read-only
            // one still submits it, so a refused save draws it again.
            let control = if f.hidden || !f.locked() {
                control
            } else if f.disabled {
                format!(r#"<fieldset class="lat-fieldset lat-field__locked" disabled>{control}</fieldset>"#)
            } else {
                format!(r#"<div class="lat-field__locked" inert aria-disabled="true">{control}</div>"#)
            };
            FieldView {
                id: f.name.clone(),
                label,
                control,
                help: f.help.as_ref().map(|h| shell.tt(h)),
                section: !pf.field_type.stores(),
                required,
                enter: f.enter.attribute(),
                layout: f.wrapper_classes(),
                attrs: behaviour_attrs(f),
                hidden: f.hidden,
                chrome: None,
                // Localize each per-field message through the request translator.
                errors: bag.messages(&f.name).iter().map(|m| shell.tt(m)).collect(),
            }
        })
        .collect();
    let fields = arrange_tabs(&form.config, fields, shell);
    // Collect the widget assets the rendered field types declare, deduped and
    // resolved to URLs for the head. Most declare none (their widgets ship in
    // core laterite.js).
    let keys: Vec<&str> = form
        .config
        .fields
        .iter()
        .zip(&form.fields)
        .filter_map(|(f, pf)| {
            state
                .field_types
                .get(&f.field_type)
                .map(|ft| ft.assets(&pf.opts))
        })
        .flatten()
        .collect();
    // Localize the title and banner before the shell is moved into the template.
    let shell_title = shell.tt(&form.config.title);
    let refused = error.is_some() || !bag.is_empty();
    let error = error.map(|m| shell.tt(&m));
    let shell_base = shell.base.clone();
    let shell_i18n = shell.clone();
    let read_only = !action.ends_with("/new")
        && form
            .update_permission
            .as_deref()
            .is_some_and(|p| !shell.allows(p));
    let mut shell = shell.clone();
    shell.assets = crate::page_assets(&keys, &shell.base, &state.assets, &state.asset_urls);
    FormTemplate {
        shell,
        title: shell_title,
        action: action.to_string(),
        cancel_path: form
            .config
            .done_path
            .clone()
            .unwrap_or_else(|| form.config.base_path.clone()),
        error,
        fields,
        stay: form.stay,
        // Save goes somewhere other than the list on this page, so closing
        // is offered beside it.
        close: if action.ends_with("/new") {
            form.config.after_save.create.is_some()
        } else {
            form.config.after_save.update.is_some()
        },
        actions: action_views(&form.config.actions, action, &shell_base, &shell_i18n),
        read_only,
        preview: false,
        edit_href: None,
        panels: Vec::new(),
        sections: Vec::new(),
        delete: None,
        linked: None,
        pending: Vec::new(),
        enter_off: form.config.enter == EnterPolicy::Off,
        refused,
        leave_off: !form.config.confirm_leave,
        focus: match form.config.focus {
            FormFocus::Off => "off",
            // A new record has nothing in it yet, so the first field is where
            // the operator starts. An edit opens for reading first.
            FormFocus::Auto if action.ends_with("/new") => "first",
            FormFocus::Auto => "",
        },
    }
}

struct FieldView {
    /// DOM id for the control and its label's `for`.
    id: String,
    label: String,
    /// The control HTML, rendered by the field-type registry (override-aware).
    /// The framework owns the surrounding chrome (label, required marker, errors).
    control: String,
    /// The line beneath the control, localized.
    help: Option<String>,
    /// A `section` field: the label is a heading over the fields after it.
    section: bool,
    required: bool,
    errors: Vec<String>,
    /// The field's own Enter rule (`off`, `next`), or empty for the form's.
    enter: &'static str,
    /// Wrapper classes for the span and a row break, each led by a space.
    layout: String,
    /// The wrapper's preset and trigger attributes, each led by a space.
    attrs: String,
    /// A hidden input with no chrome around it.
    hidden: bool,
    /// Markup written as it is in place of a field: a tab set's frame.
    chrome: Option<String>,
}

impl FieldView {
    fn chrome(html: String) -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            control: String::new(),
            help: None,
            section: false,
            required: false,
            errors: Vec::new(),
            enter: "",
            layout: String::new(),
            attrs: String::new(),
            hidden: false,
            chrome: Some(html),
        }
    }
}

/// A label as a DOM id's part: its letters and digits, lowercase, the rest `-`.
pub(crate) fn dom_slug(label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "tab".to_string()
    } else {
        slug
    }
}

/// The fields in the order the page draws them: the form's own, then each tab
/// set as a strip of tabs over a pane a tab. The set opens on `active_tab`,
/// or on the first tab holding a refused field.
fn arrange_tabs(form: &FormConfig, views: Vec<FieldView>, shell: &crate::Shell) -> Vec<FieldView> {
    if form.fields.iter().all(|f| f.area == FormArea::Outside) {
        return views;
    }
    let mut outside = Vec::new();
    // Per set: its tabs in order of first appearance, each with its fields.
    let mut sets: [Vec<(Text, Vec<FieldView>)>; 2] = [Vec::new(), Vec::new()];
    for (field, view) in form.fields.iter().zip(views) {
        let set = match field.area {
            FormArea::Outside => {
                outside.push(view);
                continue;
            }
            FormArea::Primary => &mut sets[0],
            FormArea::Secondary => &mut sets[1],
        };
        let tab = field.tab.clone().unwrap_or_else(|| Text::new("Misc"));
        match set.iter_mut().find(|(t, _)| t.source() == tab.source()) {
            Some((_, fields)) => fields.push(view),
            None => set.push((tab, vec![view])),
        }
    }
    let mut out = outside;
    let none = FormTabs::default();
    for (index, (tabs, config)) in sets
        .into_iter()
        .zip([form.tabs.as_ref(), form.secondary_tabs.as_ref()])
        .enumerate()
    {
        if tabs.is_empty() {
            continue;
        }
        let config = config.unwrap_or(&none);
        let prefix = if index == 0 { "tab" } else { "tab2" };
        let refused = tabs
            .iter()
            .position(|(_, fields)| fields.iter().any(|f| !f.errors.is_empty()));
        let asked = config.active_tab.as_deref().and_then(|want| {
            want.trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .filter(|n| *n < tabs.len())
                .or_else(|| tabs.iter().position(|(t, _)| t.source() == want.trim()))
        });
        let active = refused.or(asked).unwrap_or(0);
        let ids: Vec<String> = tabs
            .iter()
            .map(|(tab, _)| {
                config
                    .identifiers
                    .get(tab.source())
                    .cloned()
                    .unwrap_or_else(|| format!("{prefix}-{}", dom_slug(tab.source())))
            })
            .collect();
        let mut strip = String::new();
        for (i, ((tab, _), id)) in tabs.iter().zip(&ids).enumerate() {
            let icon = config
                .icons
                .get(tab.source())
                .map(|name| shell.icon(name).to_string())
                .unwrap_or_default();
            strip.push_str(&format!(
                r#"<button type="button" role="tab" class="lat-pagetabs__tab{on}" id="{id}" aria-controls="{id}-pane" aria-selected="{selected}"{tabindex}>{icon}<span>{label}</span></button>"#,
                on = if i == active { " is-active" } else { "" },
                id = escape_attr(id),
                selected = i == active,
                tabindex = if i == active { "" } else { r#" tabindex="-1""# },
                label = escape_attr(&shell.tt(tab)),
            ));
        }
        // The form's own grid closes, the set opens, and each pane is a grid.
        out.push(FieldView::chrome(format!(
            r#"</div><div class="lat-formtabs{stretch}{class}" data-lat-widget="formtabs"{linkable}><div class="lat-pagetabs" role="tablist">{strip}</div>"#,
            stretch = if config.stretch { " lat-formtabs--stretch" } else { "" },
            class = config
                .css_class
                .as_deref()
                .map(|c| format!(" {}", escape_attr(c)))
                .unwrap_or_default(),
            linkable = if config.linkable { " data-linkable" } else { "" },
        )));
        for (i, ((tab, fields), id)) in tabs.into_iter().zip(&ids).enumerate() {
            let class = config.pane_css_class.of(tab.source());
            out.push(FieldView::chrome(format!(
                r#"<div class="lat-formtabs__pane lat-fields{class}{on}" role="tabpanel" id="{id}-pane" aria-labelledby="{id}" data-label="{label}">"#,
                class = if class.is_empty() {
                    String::new()
                } else {
                    format!(" {}", escape_attr(class))
                },
                on = if i == active { " is-active" } else { "" },
                id = escape_attr(id),
                label = escape_attr(&shell.tt(&tab)),
            )));
            out.extend(fields);
            out.push(FieldView::chrome("</div>".to_string()));
        }
        // The grid the template closes.
        out.push(FieldView::chrome(
            r#"</div><div class="lat-fields">"#.to_string(),
        ));
    }
    out
}

/// One relation on an edit page: its heading, its New link and the list region.
pub(crate) struct RelationSection {
    /// The section's toolbar: its title and the New or Attach button.
    pub toolbar: crate::html::Markup,
    pub html: crate::html::Markup,
}

pub(crate) fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One of a module's buttons beside Save, resolved for this record.
struct ActionView {
    label: String,
    href: String,
    post: bool,
    confirm: Option<String>,
}

/// The form's actions for the record `action` edits; none on a new record,
/// which has no id to act on yet.
fn action_views(
    actions: &[FormAction],
    action: &str,
    base: &str,
    shell: &crate::Shell,
) -> Vec<ActionView> {
    let path = action.trim_end_matches('/');
    let path = path.strip_suffix("/edit").unwrap_or(path);
    let Some(id) = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && *s != "new")
    else {
        return Vec::new();
    };
    if action.ends_with("/new") {
        return Vec::new();
    }
    actions
        .iter()
        .filter(|a| a.permission.as_deref().is_none_or(|p| shell.allows(p)))
        .map(|a| {
            let target = a.action.replace("{id}", id);
            let href = if target.starts_with("http://") || target.starts_with("https://") {
                target
            } else {
                format!(
                    "{}/{}",
                    base.trim_end_matches('/'),
                    target.trim_start_matches('/')
                )
            };
            ActionView {
                label: shell.tt(&a.label),
                href,
                post: !a.method.eq_ignore_ascii_case("get"),
                confirm: a.confirm.as_ref().map(|c| shell.tt(c)),
            }
        })
        .collect()
}

/// Some of a form's fields, each in its wrapper, for a refresh.
#[derive(Template)]
#[template(path = "_form_field_list.html")]
struct FieldsFragment {
    fields: Vec<FieldView>,
}

/// Just the form element, for an HTMX submit that failed validation: the
/// response replaces the form in place rather than reloading the page.
#[derive(Template)]
#[template(path = "_form_fields.html")]
struct FormFragment {
    shell: crate::Shell,
    action: String,
    cancel_path: String,
    error: Option<String>,
    fields: Vec<FieldView>,
    stay: bool,
    /// Offer "Save and close".
    close: bool,
    /// The module's buttons beside Save.
    actions: Vec<ActionView>,
    /// The operator may look but not save: the fields are disabled and Save
    /// is gone.
    read_only: bool,
    /// The record's preview: values to read, with no note and no buttons.
    preview: bool,
    /// Enter never submits this form.
    enter_off: bool,
    /// The form holds a submission that was not saved.
    refused: bool,
    /// Leaving with changes unsaved does not ask.
    leave_off: bool,
    /// `first`, `off`, or empty for the default.
    focus: &'static str,
}

#[derive(Template)]
#[template(path = "form.html")]
struct FormTemplate {
    shell: crate::Shell,
    title: String,
    action: String,
    cancel_path: String,
    error: Option<String>,
    fields: Vec<FieldView>,
    /// Offer "Save and continue editing".
    stay: bool,
    /// Offer "Save and close".
    close: bool,
    /// The module's buttons beside Save.
    actions: Vec<ActionView>,
    /// The operator may look but not save.
    read_only: bool,
    /// The record's preview page; see [`PreviewConfig`].
    preview: bool,
    /// Where a preview's Edit button goes.
    edit_href: Option<String>,
    /// A preview's panels, each fetched once the page is open.
    panels: Vec<PanelView>,
    /// The related records, one section each, on an edit page.
    /// The record this one belongs to, linked above the form.
    linked: Option<LinkedView>,
    sections: Vec<RelationSection>,
    /// The record's Delete, on an edit page whose list offers one.
    delete: Option<DeleteView>,
    /// The relations a create page cannot show yet, by section title.
    pending: Vec<String>,
    /// Enter never submits this form.
    enter_off: bool,
    /// The form holds a submission that was not saved.
    refused: bool,
    /// Leaving with changes unsaved does not ask.
    leave_off: bool,
    /// `first`, `off`, or empty for the default.
    focus: &'static str,
}

/// A field as a value form renders it: the label, the control its type drew,
/// the help, and the wrapper's layout classes and behaviour attributes.
pub struct ValueFieldView {
    /// The field's label, localized.
    pub label: String,
    /// The control's DOM id, which the label points at.
    pub id: String,
    /// The field's help text, localized, when it has any.
    pub help: Option<String>,
    /// A `section` field: a heading, no control.
    pub section: bool,
    /// The control, rendered by the field's own type.
    pub control: String,
    /// Wrapper classes for the span and a row break, each led by a space.
    pub layout: String,
    /// The wrapper's preset and trigger attributes, each led by a space.
    pub attrs: String,
    /// The tab the field sits on, as authored; `None` sits above the tabs.
    pub tab: Option<Text>,
}

#[derive(Template)]
#[template(path = "_value_fields.html")]
struct ValueFieldsTemplate {
    fields: Vec<ValueFieldView>,
}

/// A form over a JSON object: the declared fields rendered with their stored
/// values, and a submission collected back through the field types into an
/// object with every declared key. The settings screens are one, a theme's
/// settings another. Built from a [`RouteCtx`](crate::routes::RouteCtx) with
/// [`value_form`](crate::routes::RouteCtx::value_form).
pub struct ValueForm {
    fields: Vec<FormField>,
    types: Arc<crate::field::FieldRegistry>,
    overrides: Arc<dyn crate::field::OverrideResolver>,
    /// What an override is scoped to (a settings code, a theme code).
    scope: String,
}

impl ValueForm {
    /// Checks every field's type is registered and its options resolve. The
    /// error names the field and the type, for a form declared at runtime.
    pub(crate) fn new(
        fields: &[FormField],
        types: Arc<crate::field::FieldRegistry>,
        overrides: Arc<dyn crate::field::OverrideResolver>,
        scope: &str,
    ) -> Result<Self, Text> {
        for field in fields {
            let Some(ft) = types.get(&field.field_type) else {
                return Err(Text::dynamic(format!(
                    "field {:?} uses unregistered type {:?}",
                    field.name, field.field_type
                )));
            };
            if ft.resolve_options(&field.options, &types).is_err() {
                return Err(Text::dynamic(format!(
                    "field {:?} has malformed options",
                    field.name
                )));
            }
        }
        Ok(Self {
            fields: fields.to_vec(),
            types,
            overrides,
            scope: scope.to_string(),
        })
    }

    /// The fields, in order.
    pub fn fields(&self) -> &[FormField] {
        &self.fields
    }

    /// Each field over `stored`, ready for a template.
    pub fn views(&self, stored: &serde_json::Value, shell: &crate::Shell) -> Vec<ValueFieldView> {
        value_views(
            &self.fields,
            &self.types,
            self.overrides.as_ref(),
            stored,
            shell,
            &self.scope,
        )
    }

    /// The fields over `stored` as one `.lat-fields` block, for a form a screen
    /// builds by hand around it.
    pub fn render(&self, stored: &serde_json::Value, shell: &crate::Shell) -> crate::html::Markup {
        let template = ValueFieldsTemplate {
            fields: self.views(stored, shell),
        };
        crate::html::Markup::from_template(&template).unwrap_or_default()
    }

    /// The asset keys the fields' types declare, for the page that shows the
    /// form: pass them to [`PageHead::assets`](crate::PageHead::assets), or a
    /// type whose widget ships apart from the shared script stays inert.
    pub fn assets(&self) -> Vec<&'static str> {
        let mut keys: Vec<&'static str> = Vec::new();
        for field in &self.fields {
            let Some(ft) = self.types.get(&field.field_type) else {
                continue;
            };
            let Ok(opts) = ft.resolve_options(&field.options, &self.types) else {
                continue;
            };
            for key in ft.assets(&opts) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// A submission as the stored object: every declared key, each value
    /// through its field type. A type that refuses its input refuses the whole.
    pub fn collect(&self, data: &HashMap<String, String>) -> Result<serde_json::Value, Text> {
        value_collect(&self.fields, &self.types, data)
    }
}

/// Each of `fields` over `stored`, rendered by its type with overrides scoped
/// to `scope`.
pub(crate) fn value_views(
    fields: &[FormField],
    types: &crate::field::FieldRegistry,
    overrides: &dyn crate::field::OverrideResolver,
    stored: &serde_json::Value,
    shell: &crate::Shell,
    scope: &str,
) -> Vec<ValueFieldView> {
    fields
        .iter()
        .map(|f| {
            let label = shell.tt(&f.label);
            let control = match types.get(&f.field_type) {
                Some(ft) => {
                    let opts = ft
                        .resolve_options(&f.options, types)
                        .unwrap_or_else(|_| ResolvedOptions::none());
                    let value = match stored.get(&f.name) {
                        Some(serde_json::Value::Array(rows)) => {
                            FieldValue::Json(serde_json::Value::Array(rows.clone()))
                        }
                        Some(serde_json::Value::String(text)) => {
                            FieldValue::Text(ft.to_control_for(&f.name, Some(text), &opts))
                        }
                        Some(serde_json::Value::Null) | None => {
                            FieldValue::Text(ft.to_control_for(&f.name, None, &opts))
                        }
                        Some(other) => FieldValue::Text(ft.to_control_for(
                            &f.name,
                            Some(&other.to_string()),
                            &opts,
                        )),
                    };
                    let cx = FieldCx {
                        name: &f.name,
                        id: &f.name,
                        label: &label,
                        value: &value,
                        required: false,
                        max_length: max_length(&f.rules),
                        opts: &opts,
                        base: &shell.base,
                        i18n: shell.i18n(),
                        icons: crate::icons::Icons::new(&shell.icon_set, &shell.sprite_url),
                    };
                    let scope = OverrideScope {
                        surface: Surface::Field,
                        view_key: &f.field_type,
                        resource: Some(scope),
                        field: Some(&f.name),
                    };
                    render_field(ft.as_ref(), overrides, &scope, &cx).into_string()
                }
                None => String::new(),
            };
            ValueFieldView {
                label,
                id: f.name.clone(),
                help: f.help.as_ref().map(|h| shell.tt(h)),
                section: f.field_type == "section",
                control: f.with_help_above(control, shell),
                layout: f.wrapper_classes(),
                attrs: behaviour_attrs(f),
                tab: f.tab.clone(),
            }
        })
        .collect()
}

/// A submission over `fields` as a JSON object with every declared key.
pub(crate) fn value_collect(
    fields: &[FormField],
    types: &crate::field::FieldRegistry,
    data: &HashMap<String, String>,
) -> Result<serde_json::Value, Text> {
    let mut object = serde_json::Map::new();
    for field in fields {
        let Some(field_type) = types.get(&field.field_type) else {
            return Err(t!("This screen uses a field type that is not registered."));
        };
        let opts = field_type
            .resolve_options(&field.options, types)
            .map_err(|_| t!("This screen has a malformed field."))?;
        let submitted = crate::field::SubmittedField::new(&field.name, data);
        // A field that supplies no value (a sealed secret shown as its mask)
        // leaves its key out, so the stored value is kept.
        let value = match field_type.to_attr(&submitted, &opts, Mode::Update) {
            Ok(Some(value)) => attr_to_json(value),
            Ok(None) => continue,
            Err(_) => return Err(t!("Check the values and try again.")),
        };
        object.insert(field.name.clone(), value);
    }
    Ok(serde_json::Value::Object(object))
}

/// A field type's attribute value as settings JSON stores it.
pub(crate) fn attr_to_json(value: laterite_core::AttrValue) -> serde_json::Value {
    use laterite_core::AttrValue;
    match value {
        AttrValue::Null => serde_json::Value::String(String::new()),
        AttrValue::Bool(b) => serde_json::Value::Bool(b),
        AttrValue::Int(i) => serde_json::Value::from(i),
        AttrValue::Float(f) => serde_json::Value::from(f),
        AttrValue::Json(j) => j,
        other => serde_json::Value::String(other.to_text().unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use laterite_core::strata::{
        async_trait, ColumnDef, CoreResult, Migration, MigrationSet, Schema, Table,
    };
    use laterite_core::testing::{connect_test, TestGuard};
    use laterite_core::Db;
    use laterite_core::SaveCx;

    /// A minimal table for exercising the generic insert/update path in isolation,
    /// defined as a portable migration so the test runs on any backend.
    struct CreateSamples;
    #[async_trait(?Send)]
    impl Migration for CreateSamples {
        fn name(&self) -> &str {
            "0001_create_samples"
        }
        async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
            s.exec(
                Table::create()
                    .table(Alias::new("samples"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("code")).text().not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    // Nullable, so a form that does not opt into timestamps
                    // still inserts.
                    .col(ColumnDef::new(Alias::new("created_at")).text())
                    .col(ColumnDef::new(Alias::new("updated_at")).text())
                    .to_owned(),
            )
            .await
        }
    }

    fn config() -> PreparedForm {
        let config = FormConfig {
            tabs: None,
            secondary_tabs: None,
            enter: Default::default(),
            confirm_leave: true,
            focus: Default::default(),
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required().unique(),
                FormField::text("name", "Name").required(),
            ],
            persist: None,
            timestamps: false,
            footprints: false,
            slugs: Default::default(),
            encrypt: Vec::new(),
            related: Default::default(),
            actions: Vec::new(),
            linked_record: None,
            after_save: AfterSave::default(),
            panels: Vec::new(),
            done_path: None,
        };
        PreparedForm::prepare(
            config,
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
            &[],
        )
        .unwrap()
    }

    /// A transient field is data on the form and nothing in the table: it
    /// round-trips through the descriptor, and loading a record leaves it
    /// empty instead of selecting a column that does not exist.
    #[test]
    fn a_transient_field_round_trips_through_the_descriptor() {
        let field = FormField::password("secret", "Secret").transient();
        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["transient"], serde_json::Value::Bool(true));
        let back: FormField = serde_json::from_value(json).unwrap();
        assert!(back.transient);
        let plain: FormField =
            serde_json::from_str(r#"{"name":"code","label":"Code","type":"text"}"#).unwrap();
        assert!(!plain.transient);
        assert!(!serde_json::to_value(&plain)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("transient"));
    }

    #[tokio::test]
    async fn loading_a_record_skips_transient_and_section_fields() {
        let (db, _guard) = test_db().await;
        sqlx::query("INSERT INTO samples (code, name) VALUES ('c1', 'Chair')")
            .execute(&db.pool)
            .await
            .unwrap();
        let form = PreparedForm::prepare(
            FormConfig {
                tabs: None,
                secondary_tabs: None,
                enter: Default::default(),
                confirm_leave: true,
                focus: Default::default(),
                entity: "samples".to_string(),
                title: "Sample".into(),
                base_path: "/admin/samples".to_string(),
                id_field: "id".to_string(),
                fields: vec![
                    FormField::of("intro", "Details", "section"),
                    FormField::text("code", "Code").required(),
                    FormField::password("secret", "Secret").transient(),
                ],
                persist: None,
                timestamps: false,
                footprints: false,
                slugs: Default::default(),
                encrypt: Vec::new(),
                related: Default::default(),
                actions: Vec::new(),
                linked_record: None,
                after_save: AfterSave::default(),
                panels: Vec::new(),
                done_path: None,
            },
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
            &[],
        )
        .unwrap();
        let values = load_values(&db, &form, "1", &[]).await.unwrap().unwrap();
        assert_eq!(values.get("code").map(String::as_str), Some("c1"));
        assert_eq!(values.get("secret").map(String::as_str), Some(""));
        // A section stores nothing, so the loader asked for no such column.
        assert_eq!(values.get("intro").map(String::as_str), Some(""));
    }

    /// The whole delete path against a real table: the row is read, the
    /// listeners run, the row goes, and the after-stage sees what went.
    #[tokio::test]
    async fn a_delete_removes_the_row_through_the_pipeline() {
        use crate::persist::{delete, DefaultPersister, DeleteRequest};

        let (db, _guard) = test_db().await;
        let st = state(db.clone());
        let form = config();

        let mut data = HashMap::new();
        data.insert("code".to_string(), "c1".to_string());
        data.insert("name".to_string(), "Chair".to_string());
        create(
            &st,
            &form,
            data,
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(
            fetch_text(&db, "name", "c1").await.as_deref(),
            Some("Chair")
        );

        let id = row_id(&db, "c1").await;
        let persister =
            DefaultPersister::from_config(&form.config, &crate::field::builtin_registry());
        let rec = delete(DeleteRequest {
            db: &db,
            cache: None,
            listeners: &[],
            persister: &persister,
            actor: &laterite_core::Actor::system("test"),
            entity: "samples",
            id: &id,
        })
        .await
        .expect("deleted");

        // Gone from the table, and the caller learns what it removed.
        assert_eq!(fetch_text(&db, "name", "c1").await, None);
        assert_eq!(rec.text("code"), Some("c1"));
    }

    /// The stored id for a row, as text, the shape the delete path takes.
    async fn row_id(db: &Db, code: &str) -> String {
        let stmt = Query::select()
            .expr_as(
                Expr::col(Alias::new("id")).cast_as(Alias::new(text_cast(db.backend))),
                Alias::new("v"),
            )
            .from(Alias::new("samples"))
            .and_where(Expr::col(Alias::new("code")).eq(code))
            .to_owned();
        let (sql, values) = laterite_core::query::build(db.backend, stmt);
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        row.get_text("v").unwrap()
    }

    pub(super) fn state(db: Db) -> AdminState {
        AdminState::new(
            laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db,
        )
    }

    /// A fresh test database holding a minimal `samples` table, on whichever
    /// backend the run targets. Hold the returned guard for the test's lifetime.
    pub(super) async fn test_db() -> (Db, TestGuard) {
        let samples = MigrationSet::new("test.samples", vec![Box::new(CreateSamples)]);
        connect_test(&[samples]).await
    }

    /// Reads a single text column from the one row matching `code`, so a test can
    /// assert what was persisted without depending on the read path under test.
    pub(super) async fn fetch_text(db: &Db, column: &str, code: &str) -> Option<String> {
        let stmt = Query::select()
            .expr_as(
                Expr::col(Alias::new(column)).cast_as(Alias::new(text_cast(db.backend))),
                Alias::new("v"),
            )
            .from(Alias::new("samples"))
            .and_where(Expr::col(Alias::new("code")).eq(code))
            .to_owned();
        let (sql, values) = to_sql(db.backend, stmt);
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(&db.pool)
            .await
            .unwrap()?;
        row.get_text_opt("v").ok().flatten()
    }

    async fn body_of(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn count(db: &Db) -> i64 {
        sqlx::query_scalar("select count(*) from samples")
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }

    fn data(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The header htmx sets on every request it makes.
    pub(super) fn htmx_headers() -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert("hx-request", "true".parse().unwrap());
        h
    }

    #[tokio::test]
    async fn create_then_fetch() {
        let (db, _guard) = test_db().await;
        let cfg = config();
        let st = state(db.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Content Editor")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        assert_eq!(
            fetch_text(&db, "name", "editor").await.as_deref(),
            Some("Content Editor")
        );
    }

    #[tokio::test]
    async fn update_changes_the_row() {
        let (db, _guard) = test_db().await;
        let cfg = config();
        let st = state(db.clone());
        create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Editor")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;

        let id = fetch_text(&db, "id", "editor")
            .await
            .expect("row should exist after create");

        // The code is unchanged, so its unique rule must ignore the edited row.
        let resp = update(
            &st,
            &cfg,
            id,
            data(&[("code", "editor"), ("name", "Senior Editor")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        assert_eq!(
            fetch_text(&db, "name", "editor").await.as_deref(),
            Some("Senior Editor")
        );
    }

    #[tokio::test]
    async fn create_re_renders_with_a_required_message_and_inserts_nothing() {
        let (db, _guard) = test_db().await;
        let cfg = config();
        let st = state(db.clone());

        let resp = create(
            &st,
            &cfg,
            data(&[("code", ""), ("name", "No Code")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        // Re-renders the form (200), does not redirect.
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 0);
        // The per-field message is rendered.
        assert!(body_of(resp).await.contains("Code is required."));
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_unique_value() {
        let (db, _guard) = test_db().await;
        let cfg = config();
        let st = state(db.clone());
        create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Editor")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;

        // A second row with the same code is refused by the unique rule.
        let resp = create(
            &st,
            &cfg,
            data(&[("code", "editor"), ("name", "Other")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 1);
        assert!(body_of(resp).await.contains("already taken"));
    }

    #[tokio::test]
    async fn text_email_input_rejects_a_bad_address_via_its_intrinsic_rule() {
        let (db, _guard) = test_db().await;
        let st = state(db.clone());
        // The `name` column is a text field with the `email` input, which
        // contributes Rule::Email without the descriptor listing it.
        let cfg = PreparedForm::prepare(
            FormConfig {
                tabs: None,
                secondary_tabs: None,
                enter: Default::default(),
                confirm_leave: true,
                focus: Default::default(),
                entity: "samples".to_string(),
                title: "Sample".into(),
                base_path: "/admin/samples".to_string(),
                id_field: "id".to_string(),
                fields: vec![
                    FormField::text("code", "Code").required(),
                    FormField::of("name", "Email", "text")
                        .options(serde_json::json!({ "input": "email" }))
                        .required(),
                ],
                persist: None,
                timestamps: false,
                footprints: false,
                slugs: Default::default(),
                encrypt: Vec::new(),
                related: Default::default(),
                actions: Vec::new(),
                linked_record: None,
                after_save: AfterSave::default(),
                panels: Vec::new(),
                done_path: None,
            },
            &st.field_types,
            &PersisterRegistry::new(),
            &[],
        )
        .unwrap();

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "c1"), ("name", "nope")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 0);

        let resp = create(
            &st,
            &cfg,
            data(&[("code", "c1"), ("name", "a@b.test")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(count(&db).await, 1);
    }

    #[test]
    fn prepare_rejects_an_unregistered_field_type() {
        let config = FormConfig {
            tabs: None,
            secondary_tabs: None,
            enter: Default::default(),
            confirm_leave: true,
            focus: Default::default(),
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![FormField::of("place", "Place", "no.such.type")],
            persist: None,
            timestamps: false,
            footprints: false,
            slugs: Default::default(),
            encrypt: Vec::new(),
            related: Default::default(),
            actions: Vec::new(),
            linked_record: None,
            after_save: AfterSave::default(),
            panels: Vec::new(),
            done_path: None,
        };
        let Err(err) = PreparedForm::prepare(
            config,
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
            &[],
        ) else {
            panic!("expected prepare to reject an unregistered type");
        };
        assert!(err.contains("no.such.type"), "{err}");
    }

    #[test]
    fn prepare_rejects_a_malformed_option() {
        // The text field's `input` option is a string; a wrong JSON type for it
        // aborts prepare naming the field.
        let config = FormConfig {
            tabs: None,
            secondary_tabs: None,
            enter: Default::default(),
            confirm_leave: true,
            focus: Default::default(),
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::of("email", "Email", "text").options(serde_json::json!({ "input": 7 }))
            ],
            persist: None,
            timestamps: false,
            footprints: false,
            slugs: Default::default(),
            encrypt: Vec::new(),
            related: Default::default(),
            actions: Vec::new(),
            linked_record: None,
            after_save: AfterSave::default(),
            panels: Vec::new(),
            done_path: None,
        };
        let Err(err) = PreparedForm::prepare(
            config,
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
            &[],
        ) else {
            panic!("expected prepare to reject a malformed option");
        };
        assert!(err.contains("`email`"), "{err}");
    }

    /// A persister that inserts a row then fails. The pipeline owns the
    /// transaction, so the insert must roll back with it.
    struct RollbackPersister;
    #[laterite_core::strata::async_trait]
    impl Persister for RollbackPersister {
        async fn create(
            &self,
            cx: &mut laterite_core::SaveCx<'_>,
            _rec: &Record,
        ) -> Result<i64, SaveError> {
            let backend = cx.backend();
            let insert = Query::insert()
                .into_table(Alias::new("samples"))
                .columns([Alias::new("code"), Alias::new("name")])
                .values_panic(["rolled".into(), "back".into()])
                .to_owned();
            let (sql, values) = to_sql(backend, insert);
            bind_values(sqlx::query(&sql), values)
                .execute(cx.conn())
                .await
                .map_err(|e| SaveError::Failed(e.to_string()))?;
            Err(SaveError::Failed("deliberate".to_string()))
        }
        async fn update(
            &self,
            _cx: &mut SaveCx<'_>,
            _id: &str,
            _rec: &Record,
        ) -> Result<(), SaveError> {
            Ok(())
        }
    }

    /// A persister that always rejects create with a per-field validation error.
    struct RejectingPersister;
    #[laterite_core::strata::async_trait]
    impl Persister for RejectingPersister {
        async fn create(&self, _cx: &mut SaveCx<'_>, _rec: &Record) -> Result<i64, SaveError> {
            let mut bag = ErrorBag::default();
            bag.add("code", t!("Code is not allowed here."));
            Err(SaveError::Invalid(bag))
        }
        async fn update(
            &self,
            _cx: &mut SaveCx<'_>,
            _id: &str,
            _rec: &Record,
        ) -> Result<(), SaveError> {
            Ok(())
        }
    }

    fn config_with(persister: &str) -> FormConfig {
        FormConfig {
            tabs: None,
            secondary_tabs: None,
            enter: Default::default(),
            confirm_leave: true,
            focus: Default::default(),
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required(),
                FormField::text("name", "Name").required(),
            ],
            persist: Some(persister.to_string()),
            timestamps: false,
            footprints: false,
            slugs: Default::default(),
            encrypt: Vec::new(),
            related: Default::default(),
            actions: Vec::new(),
            linked_record: None,
            after_save: AfterSave::default(),
            panels: Vec::new(),
            done_path: None,
        }
    }

    #[test]
    fn prepare_rejects_an_unregistered_persister() {
        let Err(err) = PreparedForm::prepare(
            config_with("no.such.persister"),
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
            &[],
        ) else {
            panic!("expected prepare to reject an unregistered persister");
        };
        assert!(err.contains("no.such.persister"), "{err}");
    }

    #[tokio::test]
    async fn a_persister_transaction_rolls_back_on_error() {
        let (db, _guard) = test_db().await;
        let st = state(db.clone());
        let mut persisters = PersisterRegistry::new();
        persisters.insert("test.rollback".to_string(), Arc::new(RollbackPersister));
        let form = PreparedForm::prepare(
            config_with("test.rollback"),
            &st.field_types,
            &persisters,
            &[],
        )
        .unwrap();

        let resp = create(
            &st,
            &form,
            data(&[("code", "rolled"), ("name", "back")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        // The persister failed, so the form re-renders as a refusal (422) and
        // its in-tx insert rolled back: no row landed.
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(count(&db).await, 0);
    }

    #[tokio::test]
    async fn a_persister_invalid_error_re_renders_422() {
        let (db, _guard) = test_db().await;
        let st = state(db.clone());
        let mut persisters = PersisterRegistry::new();
        persisters.insert("test.reject".to_string(), Arc::new(RejectingPersister));
        let form = PreparedForm::prepare(
            config_with("test.reject"),
            &st.field_types,
            &persisters,
            &[],
        )
        .unwrap();

        let resp = create(
            &st,
            &form,
            data(&[("code", "editor"), ("name", "Editor")]),
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &axum::http::HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body_of(resp).await.contains("Code is not allowed here."));
    }
}

#[cfg(test)]
mod timestamp_tests {
    use super::tests::*;
    use super::*;
    use laterite_core::listeners::{CREATED_AT, UPDATED_AT};
    use laterite_core::Actor;

    fn timestamped(on: bool) -> FormConfig {
        FormConfig {
            tabs: None,
            secondary_tabs: None,
            enter: Default::default(),
            confirm_leave: true,
            focus: Default::default(),
            entity: "samples".to_string(),
            title: "Sample".into(),
            base_path: "/admin/samples".to_string(),
            id_field: "id".to_string(),
            fields: vec![
                FormField::text("code", "Code").required(),
                FormField::text("name", "Name").required(),
            ],
            persist: None,
            timestamps: on,
            footprints: false,
            slugs: Default::default(),
            encrypt: Vec::new(),
            related: Default::default(),
            actions: Vec::new(),
            linked_record: None,
            after_save: AfterSave::default(),
            panels: Vec::new(),
            done_path: None,
        }
    }

    async fn create_through_pipeline(db: &laterite_core::Db, on: bool) -> Record {
        let form = PreparedForm::prepare(
            timestamped(on),
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
            &[],
        )
        .unwrap();
        let mut data = HashMap::new();
        data.insert("code".to_string(), "c1".to_string());
        data.insert("name".to_string(), "Chair".to_string());
        let mut rec = declared_record(&form, &data, Mode::Create).unwrap();
        let actor = Actor::system("test");
        persist::save(
            persist::SaveRequest {
                db,
                cache: None,
                listeners: &form.listeners,
                persister: form.persister.as_ref(),
                actor: &actor,
                op: Op::Create,
                id: None,
                then: None,
            },
            &mut rec,
        )
        .await
        .expect("the insert should succeed");
        rec
    }

    #[tokio::test]
    async fn opting_in_stamps_the_row() {
        let (db, _guard) = test_db().await;
        let rec = create_through_pipeline(&db, true).await;
        assert!(rec.datetime(CREATED_AT).is_some());
        assert!(rec.datetime(UPDATED_AT).is_some());
        // The stamp reached the table, not just the record.
        assert!(fetch_text(&db, CREATED_AT, "c1").await.is_some());
    }

    #[tokio::test]
    async fn leaving_it_off_writes_no_timestamps() {
        let (db, _guard) = test_db().await;
        let rec = create_through_pipeline(&db, false).await;
        assert!(rec.get(CREATED_AT).is_none());
        assert_eq!(fetch_text(&db, CREATED_AT, "c1").await, None);
    }
}

#[cfg(test)]
mod htmx_tests {
    use super::tests::*;
    use super::*;
    use axum::http::StatusCode;

    /// A submission missing a required field, so validation refuses it.
    fn invalid() -> HashMap<String, String> {
        let mut d = HashMap::new();
        d.insert("code".to_string(), String::new());
        d.insert("name".to_string(), "No code".to_string());
        d
    }

    fn valid() -> HashMap<String, String> {
        let mut d = HashMap::new();
        d.insert("code".to_string(), "c1".to_string());
        d.insert("name".to_string(), "Chair".to_string());
        d
    }

    async fn post(data: HashMap<String, String>, headers: axum::http::HeaderMap) -> Response {
        let (db, _guard) = test_db().await;
        let st = state(db);
        let form = PreparedForm::prepare(
            FormConfig {
                tabs: None,
                secondary_tabs: None,
                enter: Default::default(),
                confirm_leave: true,
                focus: Default::default(),
                entity: "samples".to_string(),
                title: "Sample".into(),
                base_path: "/admin/samples".to_string(),
                id_field: "id".to_string(),
                fields: vec![
                    FormField::text("code", "Code").required(),
                    FormField::text("name", "Name").required(),
                ],
                persist: None,
                timestamps: false,
                footprints: false,
                slugs: Default::default(),
                encrypt: Vec::new(),
                related: Default::default(),
                actions: Vec::new(),
                linked_record: None,
                after_save: AfterSave::default(),
                panels: Vec::new(),
                done_path: None,
            },
            &st.field_types,
            &PersisterRegistry::new(),
            &[],
        )
        .unwrap();
        create(
            &st,
            &form,
            data,
            crate::Shell::test(),
            &crate::audit::test_actor(),
            &crate::session::SessionHandle::from_blob(None),
            &headers,
        )
        .await
    }

    async fn body_of(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn an_htmx_submit_that_fails_returns_the_form_alone() {
        let resp = post(invalid(), htmx_headers()).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = body_of(resp).await;
        assert!(html.contains("<form"), "the form comes back");
        assert!(html.contains("lat-field__error"), "with its errors");
        // A fragment, not a page: swapping a whole document into the form would
        // nest the admin inside itself.
        assert!(!html.contains("<body"), "no page chrome");
    }

    #[tokio::test]
    async fn a_plain_submit_that_fails_returns_the_whole_page() {
        let resp = post(invalid(), axum::http::HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = body_of(resp).await;
        assert!(html.contains("<body"), "the admin still works without htmx");
        assert!(html.contains("lat-field__error"));
    }

    #[tokio::test]
    async fn an_htmx_submit_that_succeeds_redirects_by_header() {
        let resp = post(valid(), htmx_headers()).await;
        // htmx cannot follow a 303 usefully; it reads this header instead.
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(resp.headers().get("HX-Redirect").unwrap(), "/admin/samples");
    }

    #[tokio::test]
    async fn a_plain_submit_that_succeeds_still_redirects() {
        let resp = post(valid(), axum::http::HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }
}
