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
    t, Actor, AnyRowExt, ErrorBag, ModelListener, ModelListenerReg, Op, Record, Text,
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
    /// What Enter does in this field: `submit` (the form's rule), `off`, or
    /// `next` (moves to the next field).
    pub enter: FieldEnter,
    /// The share of the row the field takes, on a twelve-column grid.
    pub span: Span,
    /// Starts a new row, whatever room the current one has left.
    pub break_row: bool,
    /// Follows another field as it is typed, until this one is edited.
    pub preset: Option<Preset>,
    /// Changes when another field's state meets a condition.
    pub trigger: Option<Trigger>,
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
    pub field: String,
    pub shape: PresetShape,
}

impl Preset {
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
            enter: EnterPolicy::default(),
            confirm_leave: true,
            focus: FormFocus::default(),
            persist: None,
            timestamps: false,
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
                        "enter" => field.enter = map.next_value()?,
                        "span" => field.span = map.next_value()?,
                        "break" => field.break_row = map.next_value()?,
                        "preset" => field.preset = map.next_value()?,
                        "trigger" => field.trigger = map.next_value()?,
                        _ => return Ok(false),
                    }
                    Ok(true)
                })?;
                field.options = theirs;
                if let Some(trigger) = &field.trigger {
                    trigger.check().map_err(serde::de::Error::custom)?;
                }
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
        map.serialize_entry("enter", &self.enter)?;
        map.serialize_entry("span", &self.span)?;
        map.serialize_entry("break", &self.break_row)?;
        if let Some(preset) = &self.preset {
            map.serialize_entry("preset", preset)?;
        }
        if let Some(trigger) = &self.trigger {
            map.serialize_entry("trigger", trigger)?;
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
        self.name = key;
    }
}

impl FormField {
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
            enter: FieldEnter::default(),
            span: Span::default(),
            break_row: false,
            preset: None,
            trigger: None,
        }
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

    pub fn text(name: &str, label: impl Into<Text>) -> Self {
        Self::of(name, label, "text")
    }

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
    pub entity: String,
    /// The screen title, localized at render. Serde stays a plain string.
    pub title: Text,
    pub base_path: String,
    pub id_field: String,
    #[serde(
        deserialize_with = "crate::keyed::deserialize",
        serialize_with = "crate::keyed::serialize"
    )]
    pub fields: Vec<FormField>,
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
            persist: None,
            timestamps: false,
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

    fn idents_valid(&self) -> bool {
        valid_ident(&self.entity)
            && valid_ident(&self.id_field)
            && self.fields.iter().all(|f| valid_ident(&f.name))
    }
}

/// A [`FormConfig`] with each field's options resolved once, at router build.
/// Handlers carry this so option resolution and intrinsic-rule merging happen a
/// single time (not per render), and a malformed option or unregistered type
/// aborts boot rather than degrading silently to plain text.
pub(crate) struct PreparedForm {
    config: FormConfig,
    /// Parallel to `config.fields`.
    fields: Vec<PreparedField>,
    /// The write handler: the named persister, or the built-in default.
    persister: Arc<dyn Persister>,
    /// Listeners for this form's entity, in registration order.
    listeners: Vec<Arc<dyn ModelListener>>,
}

/// One field's boot-resolved state: its typed options and its merged rules (the
/// type's intrinsic rules ahead of the descriptor's own).
struct PreparedField {
    /// Resolved once at boot, so the save and load hooks need no lookup.
    field_type: Arc<dyn crate::field::FieldType>,
    opts: ResolvedOptions,
    rules: Vec<Rule>,
}

impl PreparedForm {
    /// Resolves every field against the registry. Returns the offending field
    /// and reason (for a boot abort) when a type is unregistered or rejects its
    /// options.
    pub(crate) fn prepare(
        config: FormConfig,
        field_types: &crate::field::FieldRegistry,
        persisters: &PersisterRegistry,
        listeners: &[ModelListenerReg],
    ) -> Result<Self, String> {
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
            let opts = ft
                .resolve_options(&f.options, field_types)
                .map_err(|e| format!("field `{}` (`{}`): {e}", f.name, f.field_type))?;
            let mut rules = ft.intrinsic_rules(&opts);
            rules.extend(f.rules.clone());
            fields.push(PreparedField {
                field_type: ft.clone(),
                opts,
                rules,
            });
        }
        let persister: Arc<dyn Persister> = match &config.persist {
            Some(key) => persisters
                .get(key)
                .cloned()
                .ok_or_else(|| format!("form names unregistered persister `{key}`"))?,
            None => Arc::new(DefaultPersister::from_config(&config)),
        };
        // The framework's own listeners run first, so a contributed one can see
        // or override what they set.
        let mut resolved: Vec<Arc<dyn ModelListener>> = Vec::new();
        if config.timestamps {
            resolved.push(Arc::new(laterite_core::Timestamps));
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
fn gathered(form: &PreparedForm, mut data: HashMap<String, String>) -> HashMap<String, String> {
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
pub(crate) fn new_form(state: &AdminState, form: &PreparedForm, shell: crate::Shell) -> Response {
    render(build(
        state,
        form,
        &format!("{}/new", form.config.base_path),
        None,
        &HashMap::new(),
        &ErrorBag::default(),
        &shell,
    ))
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
    let data = gathered(form, data);

    let bag = match validate(
        &state.db,
        &form.config.entity,
        &form.config.id_field,
        &merged_field_rules(form),
        &data,
        Mode::Create,
        None,
    )
    .await
    {
        Ok(bag) => bag,
        Err(_) => return render_error(),
    };
    if !bag.is_empty() {
        // A failed submission re-renders the form with per-field errors as 422,
        // the cross-surface "validation failure" status.
        return invalid_response(htmx, state, form, &action, None, &data, &bag, &shell);
    }

    let mut rec = match declared_record(form, &data, Mode::Create) {
        Ok(rec) => rec,
        Err(e) => {
            tracing::error!(entity = %form.config.entity, error = %e, "preparing the write failed");
            return render_error();
        }
    };
    let actor = Actor::from(user);
    match persist::save(
        persist::SaveRequest {
            db: &state.db,
            listeners: &form.listeners,
            persister: form.persister.as_ref(),
            actor: &actor,
            op: Op::Create,
            id: None,
        },
        &mut rec,
    )
    .await
    {
        // The audit entry is written by the framework's audit listener, which
        // runs on every entity after the commit.
        Ok(()) => {
            session.push_flash(crate::session::FlashLevel::Success, t!("Saved."));
            saved_response(htmx, &form.config.base_path)
        }
        // A persist-time domain check re-renders 422 with its per-field messages.
        Err(SaveError::Invalid(bag)) => {
            invalid_response(htmx, state, form, &action, None, &data, &bag, &shell)
        }
        Err(SaveError::Failed(msg)) => {
            tracing::error!(error = %msg, "admin create failed");
            render(build(
                state,
                form,
                &action,
                Some(t!("Could not save. Check the values and try again.")),
                &data,
                &ErrorBag::default(),
                &shell,
            ))
        }
    }
}

/// Renders a form populated with an existing record.
pub(crate) async fn edit_form(
    state: &AdminState,
    form: &PreparedForm,
    id: String,
    shell: crate::Shell,
) -> Response {
    if !form.config.idents_valid() {
        return render_error();
    }
    // Scope the sea-query builder so it is dropped before the await below: its
    // identifiers are reference-counted (not `Send`), and a live builder across
    // the await would make this handler's future non-`Send`.
    let (sql, values) = {
        let cast = text_cast(state.db.backend);
        let mut select = Query::select();
        for field in &form.config.fields {
            select.expr_as(
                Expr::col(Alias::new(&field.name)).cast_as(Alias::new(cast)),
                Alias::new(&field.name),
            );
        }
        select.from(Alias::new(&form.config.entity)).and_where(
            Expr::col(Alias::new(&form.config.id_field))
                .cast_as(Alias::new(cast))
                .eq(id.clone()),
        );
        to_sql(state.db.backend, select)
    };
    let row = match bind_values(sqlx::query(&sql), values)
        .fetch_optional(&state.db.pool)
        .await
    {
        Ok(row) => row,
        Err(_) => return render_error(),
    };
    let Some(row) = row else {
        return not_found();
    };

    // Each stored value goes back through its field type: what a column holds and
    // what its control shows are not always the same string.
    let values = form
        .config
        .fields
        .iter()
        .zip(&form.fields)
        .map(|(f, prepared)| {
            let stored = row.get_text_opt(f.name.as_str()).ok().flatten();
            let value = prepared
                .field_type
                .to_control(stored.as_deref(), &prepared.opts);
            (f.name.clone(), value)
        })
        .collect();

    render(build(
        state,
        form,
        &format!("{}/{}/edit", form.config.base_path, id),
        None,
        &values,
        &ErrorBag::default(),
        &shell,
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
    let data = gathered(form, data);

    let bag = match validate(
        &state.db,
        &form.config.entity,
        &form.config.id_field,
        &merged_field_rules(form),
        &data,
        Mode::Update,
        Some(&id),
    )
    .await
    {
        Ok(bag) => bag,
        Err(_) => return render_error(),
    };
    if !bag.is_empty() {
        // A failed submission re-renders the form with per-field errors as 422,
        // the cross-surface "validation failure" status.
        return invalid_response(htmx, state, form, &action, None, &data, &bag, &shell);
    }

    let mut rec = match declared_record(form, &data, Mode::Update) {
        Ok(rec) => rec,
        Err(e) => {
            tracing::error!(entity = %form.config.entity, error = %e, "preparing the write failed");
            return render_error();
        }
    };
    if let Ok(n) = id.parse::<i64>() {
        rec.set_id(n);
    }
    let actor = Actor::from(user);
    match persist::save(
        persist::SaveRequest {
            db: &state.db,
            listeners: &form.listeners,
            persister: form.persister.as_ref(),
            actor: &actor,
            op: Op::Update,
            id: Some(&id),
        },
        &mut rec,
    )
    .await
    {
        Ok(()) => {
            session.push_flash(crate::session::FlashLevel::Success, t!("Saved."));
            saved_response(htmx, &form.config.base_path)
        }
        Err(SaveError::Invalid(bag)) => {
            invalid_response(htmx, state, form, &action, None, &data, &bag, &shell)
        }
        Err(SaveError::Failed(msg)) => {
            tracing::error!(error = %msg, "admin update failed");
            render(build(
                state,
                form,
                &action,
                Some(t!("Could not save. Check the values and try again.")),
                &data,
                &ErrorBag::default(),
                &shell,
            ))
        }
    }
}

/// The 422 response for a failed save. An HTMX submit gets the form alone, which
/// replaces the form in place; anything else gets the whole page, so the admin
/// still works with scripting off.
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
            };
            // The type is registered (prepare validated it); the lookup here is
            // only to render.
            let control = match state.field_types.get(&f.field_type) {
                Some(ft) => {
                    let scope = OverrideScope {
                        surface: Surface::Field,
                        view_key: &f.field_type,
                        resource: Some(&form.config.base_path),
                        field: Some(&f.name),
                    };
                    render_field(ft.as_ref(), state.overrides.as_ref(), &scope, &cx).into_string()
                }
                None => String::new(),
            };
            FieldView {
                id: f.name.clone(),
                label,
                control,
                required,
                enter: f.enter.attribute(),
                layout: layout_classes(f.span, f.break_row),
                attrs: behaviour_attrs(f),
                // Localize each per-field message through the request translator.
                errors: bag.messages(&f.name).iter().map(|m| shell.tt(m)).collect(),
            }
        })
        .collect();
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
    let mut shell = shell.clone();
    shell.assets = crate::page_assets(&keys, &shell.base, &state.assets, &state.asset_urls);
    FormTemplate {
        shell,
        title: shell_title,
        action: action.to_string(),
        cancel_path: form.config.base_path.clone(),
        error,
        fields,
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
    required: bool,
    errors: Vec<String>,
    /// The field's own Enter rule (`off`, `next`), or empty for the form's.
    enter: &'static str,
    /// Wrapper classes for the span and a row break, each led by a space.
    layout: String,
    /// The wrapper's preset and trigger attributes, each led by a space.
    attrs: String,
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
    /// Enter never submits this form.
    enter_off: bool,
    /// The form holds a submission that was not saved.
    refused: bool,
    /// Leaving with changes unsaved does not ask.
    leave_off: bool,
    /// `first`, `off`, or empty for the default.
    focus: &'static str,
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
        };
        PreparedForm::prepare(
            config,
            &crate::field::builtin_registry(),
            &PersisterRegistry::new(),
            &[],
        )
        .unwrap()
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
        let persister = DefaultPersister::from_config(&form.config);
        let rec = delete(DeleteRequest {
            db: &db,
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
        async fn create(&self, cx: &mut SaveCx<'_>, _rec: &Record) -> Result<i64, SaveError> {
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
        // The persister failed, so the form re-renders (200) and its in-tx insert
        // rolled back: no row landed.
        assert_eq!(resp.status(), StatusCode::OK);
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
                listeners: &form.listeners,
                persister: form.persister.as_ref(),
                actor: &actor,
                op: Op::Create,
                id: None,
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
