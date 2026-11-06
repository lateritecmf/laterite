//! One validation engine for admin forms and the API.
//!
//! Typed [`Rule`]s run over a field's submitted value into an [`ErrorBag`] of
//! locale-free [`Text`] messages: the admin form reads it field by field and
//! localizes each inline, the API localizes it into a `422` body. Cheap rules
//! (required, length, format) run without touching the database; the one DB-backed
//! rule, [`Rule::Unique`], runs a single `EXISTS`-style probe and only for a field
//! that already passed its cheap rules, so a blank or malformed value is never also
//! probed.
//!
//! The engine is bespoke because Laterite's rules are runtime and
//! descriptor-driven (a `FormConfig` or YAML over a `HashMap`), which the
//! derive-macro crates (`validator`, `garde`) do not fit, and because `Unique`
//! is inherently database-specific. The one genuinely-standard primitive, email
//! shape, is delegated to the RFC-compliant `email_address` crate.

use std::collections::{BTreeMap, HashMap};

use sea_query::{Alias, Expr, Query};
use serde::{Deserialize, Serialize};

use crate::i18n::Text;
use crate::query::{bind_values, build, text_cast};
use crate::{CoreResult, Db};

/// Whether a submission creates a new record or updates an existing one. A
/// [`Rule::Unique`] check ignores the edited row on update, and
/// [`Rule::RequiredOn`] fires for one mode only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The submission inserts a new record.
    Create,
    /// The submission updates an existing record.
    Update,
}

/// A single rule applied to one field's submitted value. Serde-serialisable so
/// a descriptor (later YAML) can author rules by name (`required`, `email`,
/// `max_length`, ...).
///
/// Non-exhaustive: rules are added as field types need them, so match with a `_`
/// arm.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
    /// Parses as a number (integer or decimal). Skipped when empty.
    Numeric,
    /// A syntactically valid absolute URL with a host. Skipped when empty.
    Url,
    /// A calendar date, `YYYY-MM-DD`. Skipped when empty.
    Date,
    /// The value must not already exist in this field's column of the target
    /// table. On update, the edited row is ignored.
    Unique,
    /// A rule a module registered, by its dotted name, with the arguments the
    /// descriptor gave: `acme.vat_number`, or `acme.between: [1, 10]`.
    #[serde(untagged)]
    Custom(CustomRule),
}

/// A registered rule as a descriptor names it: a dotted `vendor.rule` name and
/// its arguments as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRule {
    /// The registered name, such as `acme.vat_number`.
    pub name: String,
    /// The arguments, each as the descriptor wrote it.
    pub args: Vec<String>,
}

impl CustomRule {
    /// The rule registered as `name`, with `args`.
    pub fn new(name: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            args,
        }
    }
}

impl Serialize for CustomRule {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        if self.args.is_empty() {
            return ser.serialize_str(&self.name);
        }
        let mut map = ser.serialize_map(Some(1))?;
        map.serialize_entry(&self.name, &self.args)?;
        map.end()
    }
}

/// One argument as a descriptor may write it.
#[derive(Deserialize)]
#[serde(untagged)]
enum ArgValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl ArgValue {
    fn into_text(self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Int(n) => n.to_string(),
            Self::Float(n) => n.to_string(),
            Self::Text(t) => t,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Args {
    Many(Vec<ArgValue>),
    One(ArgValue),
}

impl<'de> Deserialize<'de> for CustomRule {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = CustomRule;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a built-in rule or a registered `vendor.rule` name")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<CustomRule, E> {
                checked_name(name).map(|n| CustomRule::new(n, Vec::new()))
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<CustomRule, M::Error> {
                let Some((name, args)) = map.next_entry::<String, Args>()? else {
                    return Err(serde::de::Error::custom("an empty rule"));
                };
                if map.next_key::<String>()?.is_some() {
                    return Err(serde::de::Error::custom("a rule names one rule"));
                }
                let args = match args {
                    Args::Many(many) => many.into_iter().map(ArgValue::into_text).collect(),
                    Args::One(one) => vec![one.into_text()],
                };
                checked_name(&name).map(|n| CustomRule::new(n, args))
            }
        }
        de.deserialize_any(Visitor)
    }
}

/// `name` when it is a dotted `vendor.rule`, the only shape a registered rule
/// may take.
fn checked_name<E: serde::de::Error>(name: &str) -> Result<String, E> {
    match name.split_once('.') {
        Some((vendor, rule)) if !vendor.is_empty() && !rule.is_empty() => Ok(name.to_string()),
        _ => Err(E::custom(format!(
            "unknown rule `{name}`: a registered rule is named `vendor.rule`"
        ))),
    }
}

/// What a registered rule is checking: the field, its arguments, and every
/// other submitted value.
#[non_exhaustive]
pub struct RuleCx<'a> {
    /// The submitted field's name.
    pub field: &'a str,
    /// The field's label, for the message.
    pub label: &'a str,
    /// The arguments the descriptor gave the rule.
    pub args: &'a [String],
    /// Every submitted value, by field name.
    pub data: &'a HashMap<String, String>,
    /// Whether the submission creates or updates a record.
    pub mode: Mode,
    /// The database, for a rule that looks something up.
    pub db: &'a Db,
    /// The table the form writes.
    pub table: &'a str,
    /// The record being updated; `None` on a create.
    pub edited_id: Option<&'a str>,
}

/// A validation rule a module registers under a dotted name.
#[async_trait::async_trait]
pub trait ValidationRule: Send + Sync + 'static {
    /// Checks `value` (trimmed), returning the message to show on refusal.
    async fn check(&self, cx: &RuleCx<'_>, value: &str) -> Result<(), Text>;

    /// Whether the rule also runs on an empty value. Default `false`: an empty
    /// value is `required`'s to refuse.
    fn implicit(&self) -> bool {
        false
    }
}

/// A rule and the dotted name descriptors use for it.
#[derive(Clone)]
pub struct ValidationRuleReg {
    /// The name, such as `acme.vat_number`.
    pub name: String,
    /// The rule itself.
    pub rule: std::sync::Arc<dyn ValidationRule>,
}

impl ValidationRuleReg {
    /// Registers `rule` as `name`.
    pub fn new(name: impl Into<String>, rule: std::sync::Arc<dyn ValidationRule>) -> Self {
        Self {
            name: name.into(),
            rule,
        }
    }
}

/// Registered rules by name.
pub type RuleRegistry = HashMap<String, std::sync::Arc<dyn ValidationRule>>;

/// Every `(field, rule name)` in `fields` that `rules` does not hold.
pub fn unknown_rules(fields: &[FieldRules], rules: &RuleRegistry) -> Vec<(String, String)> {
    fields
        .iter()
        .flat_map(|f| {
            f.rules.iter().filter_map(move |r| match r {
                Rule::Custom(c) if !rules.contains_key(&c.name) => {
                    Some((f.field.clone(), c.name.clone()))
                }
                _ => None,
            })
        })
        .collect()
}

/// Runs each field's registered rules, for fields with no message yet, adding
/// any refusal to `bag`. A rule `rules` does not hold refuses the field.
#[allow(clippy::too_many_arguments)]
pub async fn validate_custom(
    db: &Db,
    table: &str,
    fields: &[FieldRules],
    data: &HashMap<String, String>,
    mode: Mode,
    edited_id: Option<&str>,
    rules: &RuleRegistry,
    bag: &mut ErrorBag,
) {
    for f in fields {
        if !bag.messages(&f.field).is_empty() {
            continue;
        }
        let value = data.get(&f.field).map(|s| s.trim()).unwrap_or("");
        for rule in &f.rules {
            let Rule::Custom(custom) = rule else {
                continue;
            };
            let Some(registered) = rules.get(&custom.name) else {
                tracing::error!(rule = %custom.name, field = %f.field, "validation rule not registered");
                bag.add(
                    &f.field,
                    field_msg("{label} could not be checked.", &f.label),
                );
                break;
            };
            if value.is_empty() && !registered.implicit() {
                continue;
            }
            let cx = RuleCx {
                field: &f.field,
                label: &f.label,
                args: &custom.args,
                data,
                mode,
                db,
                table,
                edited_id,
            };
            if let Err(message) = registered.check(&cx, value).await {
                bag.add(&f.field, message);
                break;
            }
        }
    }
}

/// The rules for one field, with the label used in its messages.
#[derive(Debug, Clone)]
pub struct FieldRules {
    /// Name of the submitted field the rules apply to.
    pub field: String,
    /// Human-readable field name the error messages use.
    pub label: String,
    /// Rules checked against the field's value, in order.
    pub rules: Vec<Rule>,
}

impl FieldRules {
    /// Returns the rules `rules` for `field`, labelled `label` in messages.
    pub fn new(field: impl Into<String>, label: impl Into<String>, rules: Vec<Rule>) -> Self {
        Self {
            field: field.into(),
            label: label.into(),
            rules,
        }
    }
}

/// Per-field validation messages, each a locale-free [`Text`]. Empty means valid.
/// Whoever renders the bag localizes each message at its sink: the admin form
/// through the request translator, the API into a localized `422` body.
#[derive(Debug, Default, Clone)]
pub struct ErrorBag {
    fields: BTreeMap<String, Vec<Text>>,
}

impl ErrorBag {
    /// Whether every field validated (no messages).
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Records a message against a field.
    pub fn add(&mut self, field: &str, message: Text) {
        self.fields
            .entry(field.to_string())
            .or_default()
            .push(message);
    }

    /// The messages recorded for `field` (empty if it validated).
    pub fn messages(&self, field: &str) -> &[Text] {
        self.fields.get(field).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every field with messages, in field order, each with its messages.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[Text])> {
        self.fields
            .iter()
            .map(|(field, messages)| (field.as_str(), messages.as_slice()))
    }
}

/// Whether the value is a calendar date in the one format a date control sends
/// and the framework stores.
fn is_date(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

/// A validation message carrying the field `label` as a nested [`Text`] argument,
/// so the label localizes too once descriptor labels become `Text`.
fn field_msg(source: &'static str, label: &str) -> Text {
    Text::new(source).arg("label", Text::dynamic(label))
}

/// A count-pluralized validation message carrying the field `label` and count `n`.
fn field_plural_msg(one: &'static str, other: &'static str, n: usize, label: &str) -> Text {
    let n = n as i64;
    Text::new(one)
        .with_plural(other, n)
        .arg("n", n)
        .arg("label", Text::dynamic(label))
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
                    bag.add(&f.field, field_msg("{label} is required.", &f.label));
                }
                Rule::RequiredOn(m) if *m == mode && trimmed.is_empty() => {
                    bag.add(&f.field, field_msg("{label} is required.", &f.label));
                }
                Rule::MinLength(n) if !trimmed.is_empty() && len < *n => {
                    bag.add(
                        &f.field,
                        field_plural_msg(
                            "{label} must be at least {n} character.",
                            "{label} must be at least {n} characters.",
                            *n,
                            &f.label,
                        ),
                    );
                }
                Rule::MaxLength(n) if len > *n => {
                    bag.add(
                        &f.field,
                        field_plural_msg(
                            "{label} must be at most {n} character.",
                            "{label} must be at most {n} characters.",
                            *n,
                            &f.label,
                        ),
                    );
                }
                Rule::Email if !trimmed.is_empty() && !is_email(trimmed) => {
                    bag.add(
                        &f.field,
                        field_msg("{label} must be a valid email address.", &f.label),
                    );
                }
                Rule::Numeric if !trimmed.is_empty() && trimmed.parse::<f64>().is_err() => {
                    bag.add(&f.field, field_msg("{label} must be a number.", &f.label));
                }
                Rule::Url if !trimmed.is_empty() && !is_url(trimmed) => {
                    bag.add(
                        &f.field,
                        field_msg("{label} must be a valid URL.", &f.label),
                    );
                }
                Rule::Date if !trimmed.is_empty() && !is_date(trimmed) => {
                    bag.add(&f.field, field_msg("{label} must be a date.", &f.label));
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
            bag.add(&f.field, field_msg("{label} is already taken.", &f.label));
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

/// Whether `s` is a syntactically valid absolute URL with a host, delegated to
/// the `url` crate. Scheme-agnostic: an input type may narrow to http(s).
fn is_url(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| u.has_host())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Translator;

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
        let tr = Translator::new("en");
        assert!(!bag.is_empty());
        assert_eq!(bag.messages("name").len(), 1); // MaxLength only (Required passed)
        assert!(tr.t(&bag.messages("name")[0]).contains("at most 5"));
        assert_eq!(bag.messages("email").len(), 1);
        assert!(tr.t(&bag.messages("email")[0]).contains("valid email"));
    }

    #[test]
    fn numeric_rejects_non_numbers_and_skips_blanks() {
        let fields = [FieldRules::new("qty", "Qty", vec![Rule::Numeric])];
        assert!(validate_fields(&fields, &data(&[("qty", "12.5")]), Mode::Create).is_empty());
        assert!(validate_fields(&fields, &data(&[("qty", "")]), Mode::Create).is_empty());
        let bag = validate_fields(&fields, &data(&[("qty", "abc")]), Mode::Create);
        assert_eq!(bag.messages("qty").len(), 1);
    }

    #[test]
    fn url_rejects_non_urls_and_skips_blanks() {
        let fields = [FieldRules::new("site", "Site", vec![Rule::Url])];
        assert!(validate_fields(
            &fields,
            &data(&[("site", "https://example.com/x")]),
            Mode::Create
        )
        .is_empty());
        // Blank is skipped (pair with Required to forbid a missing value).
        assert!(validate_fields(&fields, &data(&[("site", "")]), Mode::Create).is_empty());
        // Schemeless / relative values fail.
        let bag = validate_fields(&fields, &data(&[("site", "example.com")]), Mode::Create);
        assert_eq!(bag.messages("site").len(), 1);
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
    fn error_bag_records_messages_localized_at_the_sink() {
        let mut bag = ErrorBag::default();
        bag.add("name", field_msg("{label} is required.", "Name"));
        assert_eq!(bag.messages("name").len(), 1);
        // The bag holds a locale-free Text; the sink localizes it.
        let tr = Translator::new("en");
        assert_eq!(tr.t(&bag.messages("name")[0]), "Name is required.");
    }

    #[test]
    fn min_length_message_pluralizes_on_the_count() {
        let tr = Translator::new("en");
        assert_eq!(
            tr.t(&field_plural_msg(
                "{label} must be at least {n} character.",
                "{label} must be at least {n} characters.",
                1,
                "Pin",
            )),
            "Pin must be at least 1 character."
        );
        assert_eq!(
            tr.t(&field_plural_msg(
                "{label} must be at least {n} character.",
                "{label} must be at least {n} characters.",
                8,
                "Pin",
            )),
            "Pin must be at least 8 characters."
        );
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

    #[test]
    fn a_dotted_name_or_map_reads_as_a_registered_rule() {
        let rules: Vec<Rule> = serde_saphyr::from_str(
            "[required, acme.vat_number, {acme.between: [1, 10]}, {acme.prefix: GB}, {max_length: 5}]",
        )
        .unwrap();
        assert!(matches!(rules[0], Rule::Required));
        assert!(
            matches!(&rules[1], Rule::Custom(c) if c.name == "acme.vat_number" && c.args.is_empty())
        );
        assert!(
            matches!(&rules[2], Rule::Custom(c) if c.name == "acme.between" && c.args == ["1", "10"])
        );
        assert!(matches!(&rules[3], Rule::Custom(c) if c.args == ["GB"]));
        assert!(matches!(rules[4], Rule::MaxLength(5)));
    }

    #[test]
    fn an_unknown_undotted_rule_is_refused() {
        assert!(serde_saphyr::from_str::<Vec<Rule>>("[requird]").is_err());
    }

    #[test]
    fn a_registered_rule_writes_back_as_it_was_read() {
        let rule = Rule::Custom(CustomRule::new(
            "acme.between",
            vec!["1".into(), "10".into()],
        ));
        assert_eq!(
            serde_json::to_string(&rule).unwrap(),
            r#"{"acme.between":["1","10"]}"#
        );
        let bare = Rule::Custom(CustomRule::new("acme.vat_number", Vec::new()));
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#""acme.vat_number""#
        );
    }
}
