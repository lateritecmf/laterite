//! One validation engine for admin forms and the API.
//!
//! Typed [`Rule`]s run over a field's submitted value into an [`ErrorBag`]: the
//! admin form reads it field by field to render messages inline, and the API
//! serialises it as a `422` body. Cheap rules (required, length, format) run
//! without touching the database; the one DB-backed rule, [`Rule::Unique`], runs
//! a single `EXISTS`-style probe and only for a field that already passed its
//! cheap rules, so a blank or malformed value is never also probed.
//!
//! The engine is bespoke because Laterite's rules are runtime and
//! descriptor-driven (a `FormConfig` or YAML over a `HashMap`), which the
//! derive-macro crates (`validator`, `garde`) do not fit, and because `Unique`
//! is inherently database-specific. The one genuinely-standard primitive, email
//! shape, is delegated to the RFC-compliant `email_address` crate.

use std::collections::{BTreeMap, HashMap};

use sea_query::{Alias, Expr, Query};
use serde::{Deserialize, Serialize};

use crate::query::{bind_values, build, text_cast};
use crate::{CoreResult, Db};

/// Whether a submission creates a new record or updates an existing one. A
/// [`Rule::Unique`] check ignores the edited row on update, and
/// [`Rule::RequiredOn`] fires for one mode only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Create,
    Update,
}

/// A single rule applied to one field's submitted value. Serde-serialisable so
/// a descriptor (later YAML) can author rules by name (`required`, `email`,
/// `max_length`, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// Non-empty after trimming, in every mode.
    Required,
    /// Non-empty after trimming, in the given mode only.
    RequiredOn(Mode),
    /// At least `n` characters. Skipped when empty, so pair it with `Required`
    /// to forbid blanks.
    MinLength(usize),
    /// At most `n` characters.
    MaxLength(usize),
    /// A syntactically plausible email address. Skipped when empty.
    Email,
    /// The value must not already exist in this field's column of the target
    /// table. On update, the edited row is ignored.
    Unique,
}

/// The rules for one field, with the label used in its messages.
#[derive(Debug, Clone)]
pub struct FieldRules {
    pub field: String,
    pub label: String,
    pub rules: Vec<Rule>,
}

impl FieldRules {
    pub fn new(field: impl Into<String>, label: impl Into<String>, rules: Vec<Rule>) -> Self {
        Self {
            field: field.into(),
            label: label.into(),
            rules,
        }
    }
}

/// Per-field validation messages. Empty means valid. Serialises as a
/// `{ field: [messages] }` object (an API `422` body); the admin form reads it a
/// field at a time.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ErrorBag {
    #[serde(flatten)]
    fields: BTreeMap<String, Vec<String>>,
}

impl ErrorBag {
    /// Whether every field validated (no messages).
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Records a message against a field.
    pub fn add(&mut self, field: &str, message: impl Into<String>) {
        self.fields
            .entry(field.to_string())
            .or_default()
            .push(message.into());
    }

    /// The messages recorded for `field` (empty if it validated).
    pub fn messages(&self, field: &str) -> &[String] {
        self.fields.get(field).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Runs the cheap, non-database rules over `data` for `mode`, returning the error
/// bag. [`Rule::Unique`] needs the database and is ignored here; use [`validate`]
/// to include it. Exposed on its own for callers that want a DB-free check (for
/// example an API validating a payload before any query).
pub fn validate_fields(
    fields: &[FieldRules],
    data: &HashMap<String, String>,
    mode: Mode,
) -> ErrorBag {
    let mut bag = ErrorBag::default();
    for f in fields {
        let value = data.get(&f.field).map(String::as_str).unwrap_or("");
        let trimmed = value.trim();
        let len = trimmed.chars().count();
        for rule in &f.rules {
            match rule {
                Rule::Required if trimmed.is_empty() => {
                    bag.add(&f.field, format!("{} is required.", f.label));
                }
                Rule::RequiredOn(m) if *m == mode && trimmed.is_empty() => {
                    bag.add(&f.field, format!("{} is required.", f.label));
                }
                Rule::MinLength(n) if !trimmed.is_empty() && len < *n => {
                    bag.add(
                        &f.field,
                        format!("{} must be at least {n} characters.", f.label),
                    );
                }
                Rule::MaxLength(n) if len > *n => {
                    bag.add(
                        &f.field,
                        format!("{} must be at most {n} characters.", f.label),
                    );
                }
                Rule::Email if !trimmed.is_empty() && !is_email(trimmed) => {
                    bag.add(
                        &f.field,
                        format!("{} must be a valid email address.", f.label),
                    );
                }
                _ => {}
            }
        }
    }
    bag
}

/// Runs [`validate_fields`], then the DB-backed [`Rule::Unique`] probe for each
/// field that carries it, has a value, and passed its cheap rules. `edited_id` is
/// the row being updated (ignored by the probe); pass `None` on create.
pub async fn validate(
    db: &Db,
    table: &str,
    id_field: &str,
    fields: &[FieldRules],
    data: &HashMap<String, String>,
    mode: Mode,
    edited_id: Option<&str>,
) -> CoreResult<ErrorBag> {
    let mut bag = validate_fields(fields, data, mode);
    for f in fields {
        let wants_unique = f.rules.iter().any(|r| matches!(r, Rule::Unique));
        if !wants_unique || !bag.messages(&f.field).is_empty() {
            continue;
        }
        let value = data.get(&f.field).map(|s| s.trim()).unwrap_or("");
        if value.is_empty() {
            continue;
        }
        if unique_conflict(db, table, &f.field, value, id_field, mode, edited_id).await? {
            bag.add(&f.field, format!("{} is already taken.", f.label));
        }
    }
    Ok(bag)
}

/// Whether `value` already exists in `table.column`, ignoring the edited row on
/// update. Identifiers go through sea-query (quoted); the value and id are bound.
async fn unique_conflict(
    db: &Db,
    table: &str,
    column: &str,
    value: &str,
    id_field: &str,
    mode: Mode,
    edited_id: Option<&str>,
) -> CoreResult<bool> {
    let (sql, values) = {
        let mut stmt = Query::select();
        stmt.expr(Expr::val(1))
            .from(Alias::new(table))
            .and_where(Expr::col(Alias::new(column)).eq(value))
            .limit(1);
        if mode == Mode::Update {
            if let Some(id) = edited_id {
                stmt.and_where(
                    Expr::col(Alias::new(id_field))
                        .cast_as(Alias::new(text_cast(db.backend)))
                        .ne(id),
                );
            }
        }
        build(db.backend, stmt.to_owned())
    };
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(row.is_some())
}

/// Whether `s` is a syntactically valid email address, delegated to the
/// RFC-compliant `email_address` crate. Deliverability is confirmed by a
/// confirmation email, not by this shape check.
fn is_email(s: &str) -> bool {
    email_address::EmailAddress::is_valid(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn cheap_rules_collect_per_field_messages() {
        let fields = vec![
            FieldRules::new("name", "Name", vec![Rule::Required, Rule::MaxLength(5)]),
            FieldRules::new("email", "Email", vec![Rule::Email]),
        ];
        let bag = validate_fields(
            &fields,
            &data(&[("name", "toolong"), ("email", "nope")]),
            Mode::Create,
        );
        assert!(!bag.is_empty());
        assert_eq!(bag.messages("name").len(), 1); // MaxLength only (Required passed)
        assert!(bag.messages("name")[0].contains("at most 5"));
        assert_eq!(bag.messages("email").len(), 1);
        assert!(bag.messages("email")[0].contains("valid email"));
    }

    #[test]
    fn required_on_fires_only_in_its_mode() {
        let fields = vec![FieldRules::new(
            "password",
            "Password",
            vec![Rule::RequiredOn(Mode::Create)],
        )];
        let blank = data(&[("password", "")]);
        assert!(!validate_fields(&fields, &blank, Mode::Create).is_empty());
        assert!(validate_fields(&fields, &blank, Mode::Update).is_empty());
    }

    #[test]
    fn valid_input_produces_an_empty_bag() {
        let fields = vec![FieldRules::new(
            "email",
            "Email",
            vec![Rule::Required, Rule::Email],
        )];
        assert!(validate_fields(&fields, &data(&[("email", "a@b.com")]), Mode::Create).is_empty());
    }

    #[test]
    fn error_bag_serialises_as_field_to_messages() {
        let mut bag = ErrorBag::default();
        bag.add("name", "Name is required.");
        let json = serde_json::to_string(&bag).unwrap();
        assert_eq!(json, r#"{"name":["Name is required."]}"#);
    }

    #[test]
    fn email_shape_check_is_wired_to_the_crate() {
        // Unambiguous cases the email_address crate agrees on either way.
        assert!(is_email("a@b.com"));
        assert!(is_email("user.name+tag@sub.example.co"));
        assert!(!is_email("no-at"));
        assert!(!is_email("@b.com"));
        assert!(!is_email("a@@b.com"));
    }
}
