//! Abilities: what a module says can be done, in a form anything may call.
//!
//! An ability is one act with a name, a typed input and a permission: "add a
//! lead", "publish a version", "find a file". A screen does it through a form;
//! an ability does the same act for a caller that has no form in front of it:
//! the command palette, an assistant, an outside agent, a schedule.
//!
//! ```ignore
//! registry.add_ability(
//!     Ability::new("acme.crm.add_lead", t!("Add a lead"), Arc::new(AddLead))
//!         .describe(t!("Adds a person to the leads list."))
//!         .permission("acme.crm.leads.create")
//!         .input::<AddLeadInput>()
//!         .effect(Effect::Create),
//! );
//! ```
//!
//! The caller never decides who is acting. [`Abilities::run`] takes the
//! signed-in person, shows them only what their permissions allow, checks the
//! input against the declared schema, runs the handler as them and writes the
//! audit entry.
//!
//! How far something acting for a person may go is a [`Policy`]: each ability
//! answers [`Confirm::Auto`], [`Confirm::Ask`] or [`Confirm::Off`], by its
//! effect unless an administrator's rule or the person's own says otherwise.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use laterite_auth::{AuditEntry, AuthenticatedUser};
use laterite_core::serde_json::{json, Value};
use laterite_core::strata::async_trait;
use laterite_core::{Actor, Db, Registry, Text};

use crate::routes::RouteCtx;

/// What running an ability does to the site, which decides how carefully a
/// caller treats it: a read may simply run, a change is shown first.
///
/// Non-exhaustive so a further kind stays additive; a caller treats one it
/// does not know as it treats [`Effect::Run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Effect {
    /// Answers a question and changes nothing.
    Read,
    /// Adds a record.
    Create,
    /// Changes a record that exists.
    Update,
    /// Removes a record.
    Delete,
    /// Sends something to a person outside the panel: a message, a mail.
    Send,
    /// Anything else that changes state: publishing, importing, a rebuild.
    Run,
}

impl Effect {
    /// The effect as the lowercase word a schema or a log carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Create => "create",
            Effect::Update => "update",
            Effect::Delete => "delete",
            Effect::Send => "send",
            Effect::Run => "run",
        }
    }

    /// Whether running it changes anything.
    pub fn changes(self) -> bool {
        self != Effect::Read
    }
}

/// A link to something in the panel: a record an ability found or made.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct Link {
    /// What the link is called.
    pub label: String,
    /// Where it goes, a path inside the panel.
    pub href: String,
}

impl Link {
    /// A link called `label` to `href`.
    pub fn new(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
        }
    }
}

/// A resource's form, filled in for the person to look over and save
/// themselves: what a create or an update offers in place of doing it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct Draft {
    /// The resource's path as its descriptor declares it (`/posts`).
    pub path: String,
    /// The record being edited; `None` for a new one.
    pub id: Option<i64>,
    /// Each field's value under the field's name, as the form submits it.
    pub values: HashMap<String, String>,
}

impl Draft {
    /// A filled form for the resource at `path`: a new record, or the record
    /// `id` with `values` laid over it.
    pub fn new(path: impl Into<String>, id: Option<i64>, values: HashMap<String, String>) -> Self {
        Self {
            path: path.into(),
            id,
            values,
        }
    }
}

/// What an ability would do with an input, said before it runs.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[non_exhaustive]
pub struct Preview {
    /// The act in a sentence: "Publishes version 3 of Terms of Use."
    pub summary: Option<Text>,
    /// The records it touches, named.
    pub records: Vec<Link>,
    /// How many records it touches, when that is more than the ones named.
    pub count: Option<u64>,
    /// The form to hand the person in place of running.
    pub draft: Option<Draft>,
}

impl Preview {
    /// A preview that says `summary`.
    pub fn new(summary: Text) -> Self {
        Self {
            summary: Some(summary),
            ..Self::default()
        }
    }

    /// Names a record the act touches.
    pub fn record(mut self, link: Link) -> Self {
        self.records.push(link);
        self
    }

    /// States how many records the act touches.
    pub fn count(mut self, count: u64) -> Self {
        self.count = Some(count);
        self
    }

    /// Offers a filled form for the person to save.
    pub fn draft(mut self, draft: Draft) -> Self {
        self.draft = Some(draft);
        self
    }
}

/// What an ability did.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[non_exhaustive]
pub struct Outcome {
    /// The result in a sentence: "Added Asha Rao to the leads."
    pub message: Text,
    /// The records it found or made.
    pub links: Vec<Link>,
    /// The result as data, for a caller that reads it: the rows a search
    /// found, the figure a report holds.
    pub data: Option<Value>,
    /// This result carries text written by people outside the panel, whatever
    /// the ability declared. A caller must not take it as an instruction.
    pub untrusted: bool,
    /// A page to take the person to, under the site's own origin: a list
    /// opened with its filters set.
    pub go: Option<String>,
}

impl Outcome {
    /// An outcome that says `message`.
    pub fn new(message: Text) -> Self {
        Self {
            message,
            links: Vec::new(),
            data: None,
            untrusted: false,
            go: None,
        }
    }

    /// Takes the person to `href`, a path on this site. The palette follows
    /// it; a caller with no screen reads it as a link.
    pub fn go(mut self, href: impl Into<String>) -> Self {
        self.go = Some(href.into());
        self
    }

    /// Marks this result as carrying text written by outsiders.
    pub fn untrusted(mut self) -> Self {
        self.untrusted = true;
        self
    }

    /// Adds a link to a record.
    pub fn link(mut self, link: Link) -> Self {
        self.links.push(link);
        self
    }

    /// Carries the result as data.
    pub fn data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// Why an ability did not run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AbilityError {
    /// No ability has that code, or the person may not use it. The two are
    /// one answer, so asking tells nobody what exists.
    #[error("no such ability: {0}")]
    Unknown(String),
    /// The input does not fit the declared schema. One message per fault,
    /// each naming where in the input it is.
    #[error("the input was refused: {}", .0.join("; "))]
    Invalid(Vec<String>),
    /// The handler declined, with a reason the person can read: the record is
    /// locked, the version is already published.
    #[error("refused")]
    Refused(Text),
    /// The handler failed. The message is for the log, never for the page.
    #[error("{0}")]
    Failed(String),
}

impl AbilityError {
    /// A single fault in the input.
    pub fn invalid(message: impl Into<String>) -> Self {
        AbilityError::Invalid(vec![message.into()])
    }
}

/// What a handler is given: the person, and the panel's services.
///
/// Fields are private and read through accessors, so this can gain context.
pub struct AbilityCx<'a> {
    ctx: &'a RouteCtx,
    user: &'a AuthenticatedUser,
    actor: Actor,
}

impl<'a> AbilityCx<'a> {
    /// The panel's services: the database, forms, mail, the queue.
    pub fn ctx(&self) -> &'a RouteCtx {
        self.ctx
    }

    /// The database.
    pub fn db(&self) -> &'a Db {
        self.ctx.db()
    }

    /// The signed-in person the ability runs as.
    pub fn user(&self) -> &'a AuthenticatedUser {
        self.user
    }

    /// The person as the actor of a write, carrying the channel they act
    /// through. Pass it to [`crate::form::FormHandle`] and to
    /// [`AuditEntry::new`], so the trail says who and through what.
    pub fn actor(&self) -> &Actor {
        &self.actor
    }
}

/// The code behind an ability.
#[async_trait]
pub trait AbilityHandler: Send + Sync + 'static {
    /// What [`run`](Self::run) would do with `input`, with nothing changed.
    /// The default says nothing, which suits a read.
    async fn preview(&self, cx: &AbilityCx<'_>, input: &Value) -> Result<Preview, AbilityError> {
        let _ = (cx, input);
        Ok(Preview::default())
    }

    /// Does it. `input` has passed the declared schema, and the person holds
    /// the declared permission.
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError>;
}

/// One thing a module can do, declared so anything may call it.
#[derive(Clone)]
pub struct Ability {
    code: String,
    label: Text,
    describe: Option<Text>,
    group: Option<String>,
    permission: Option<String>,
    input: Value,
    effect: Effect,
    untrusted: bool,
    sensitive: Vec<String>,
    handler: Arc<dyn AbilityHandler>,
}

impl Ability {
    /// An ability named `code` (dotted, the module's id first:
    /// `acme.crm.add_lead`), shown to people as `label` and done by
    /// `handler`. It takes no input and has [`Effect::Run`] until told
    /// otherwise.
    pub fn new(code: impl Into<String>, label: Text, handler: Arc<dyn AbilityHandler>) -> Self {
        Self {
            code: code.into(),
            label,
            describe: None,
            group: None,
            permission: None,
            input: empty_input(),
            effect: Effect::Run,
            untrusted: false,
            sensitive: Vec::new(),
            handler,
        }
    }

    /// What it does and when to use it, written for a caller choosing among
    /// abilities.
    pub fn describe(mut self, text: Text) -> Self {
        self.describe = Some(text);
        self
    }

    /// The shelf it sits on in a catalogue. Defaults to the code without its
    /// last segment (`acme.crm`).
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The permission a person needs to see and run it. An ability that
    /// changes anything must name one.
    pub fn permission(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }

    /// The input as a type: its schema is declared, and the handler reads the
    /// same type back with `serde_json::from_value`.
    pub fn input<T: schemars::JsonSchema>(mut self) -> Self {
        let generator = schemars::generate::SchemaSettings::draft2020_12()
            .with(|s| s.inline_subschemas = true)
            .into_generator();
        self.input = generator.into_root_schema_for::<T>().to_value();
        self
    }

    /// The input as a JSON Schema object written by hand.
    pub fn input_schema(mut self, schema: Value) -> Self {
        self.input = schema;
        self
    }

    /// What running it does. See [`Effect`].
    pub fn effect(mut self, effect: Effect) -> Self {
        self.effect = effect;
        self
    }

    /// Marks the result as carrying text written by people outside the panel
    /// (a comment, a submission, a message), which a caller must never take
    /// as an instruction.
    pub fn untrusted(mut self) -> Self {
        self.untrusted = true;
        self
    }

    /// Names input fields whose values stay out of the audit entry and out of
    /// anything a caller sends elsewhere.
    pub fn sensitive<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.sensitive = fields.into_iter().map(Into::into).collect();
        self
    }

    /// The dotted code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The name people see.
    pub fn label(&self) -> &Text {
        &self.label
    }

    /// What it does, for a caller choosing among abilities.
    pub fn description(&self) -> Option<&Text> {
        self.describe.as_ref()
    }

    /// The shelf it sits on.
    pub fn group_code(&self) -> &str {
        match &self.group {
            Some(group) => group,
            None => self.code.rsplit_once('.').map_or(&*self.code, |(g, _)| g),
        }
    }

    /// The permission it asks for.
    pub fn permission_code(&self) -> Option<&str> {
        self.permission.as_deref()
    }

    /// The input's JSON Schema.
    pub fn input_schema_value(&self) -> &Value {
        &self.input
    }

    /// What running it does.
    pub fn effect_kind(&self) -> Effect {
        self.effect
    }

    /// Whether its result carries text written by outsiders.
    pub fn is_untrusted(&self) -> bool {
        self.untrusted
    }

    /// The input fields kept out of logs.
    pub fn sensitive_fields(&self) -> &[String] {
        &self.sensitive
    }

    fn allowed(&self, user: &AuthenticatedUser) -> bool {
        self.permission
            .as_deref()
            .is_none_or(|p| user.permissions.allows(p))
    }
}

impl std::fmt::Debug for Ability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ability")
            .field("code", &self.code)
            .field("effect", &self.effect)
            .field("permission", &self.permission)
            .finish_non_exhaustive()
    }
}

fn empty_input() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// A code is dotted lowercase words, two segments or more.
fn valid_code(code: &str) -> bool {
    let mut segments = 0;
    for segment in code.split('.') {
        segments += 1;
        let mut chars = segment.chars();
        if !chars.next().is_some_and(|c| c.is_ascii_lowercase())
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return false;
        }
    }
    segments >= 2
}

/// Refuses a set of abilities a caller could not rely on: a malformed or
/// repeated code, an input that is no object schema, a change nobody guards.
pub(crate) fn check(registry: &Registry) -> Result<(), String> {
    let mut seen: HashSet<&str> = crate::records::ABILITIES
        .iter()
        .map(Ability::code)
        .collect();
    for ability in registry.items::<Ability>() {
        let code = ability.code();
        if !valid_code(code) {
            return Err(format!(
                "the ability `{code}`: a code is dotted lowercase words with its module first (`acme.crm.add_lead`)"
            ));
        }
        if !seen.insert(code) {
            return Err(format!("the ability `{code}` is registered twice"));
        }
        if ability.effect.changes() && ability.permission.is_none() {
            return Err(format!(
                "the ability `{code}` changes something and names no permission"
            ));
        }
        if ability.input.get("type").and_then(Value::as_str) != Some("object") {
            return Err(format!(
                "the ability `{code}`: its input schema must be an object"
            ));
        }
        if let Err(e) = jsonschema::validator_for(&ability.input) {
            return Err(format!(
                "the ability `{code}`: its input schema is invalid: {e}"
            ));
        }
    }
    Ok(())
}

/// How far something acting for a person may go with one ability.
///
/// Ordered from the tightest to the loosest, so the tighter of two is their
/// `min`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Confirm {
    /// Not offered: the ability does not exist for anything acting for the
    /// person.
    Off,
    /// Shown to the person first, with its preview, and run when they agree.
    Ask,
    /// Run without asking.
    Auto,
}

impl Confirm {
    /// The stable name, as a rule stores it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ask => "ask",
            Self::Auto => "auto",
        }
    }

    /// What an ability answers when no rule names it: a read runs, a delete
    /// and a send are not offered, every other change asks.
    pub fn for_effect(effect: Effect) -> Self {
        match effect {
            Effect::Read => Self::Auto,
            Effect::Delete | Effect::Send => Self::Off,
            _ => Self::Ask,
        }
    }
}

/// One rule of a [`Policy`]: what it covers and how far that may go.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PolicyRule {
    /// An ability's code (`acme.crm.add_lead`), the start of one up to a dot
    /// (`acme.crm`), or an effect as `effect:create`.
    pub what: String,
    /// How far what it covers may go.
    pub confirm: Confirm,
}

impl PolicyRule {
    /// A rule for `what`: a code, the start of one, or `effect:<name>`.
    pub fn new(what: impl Into<String>, confirm: Confirm) -> Self {
        Self {
            what: what.into(),
            confirm,
        }
    }
}

/// The rules an administrator sets for the whole panel, stored as the
/// setting `backend.abilities`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PanelPolicy {
    /// The rules; the one that names an ability most exactly decides.
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

impl crate::settings::SettingsModel for PanelPolicy {
    const CODE: &'static str = POLICY_CODE;
}

/// The rules a person sets for themselves, stored among their preferences.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OwnPolicy {
    /// The rules; the one that names an ability most exactly decides.
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

impl crate::settings::PreferencesModel for OwnPolicy {
    const CODE: &'static str = OWN_POLICY_CODE;
}

/// The settings code of the panel's rules.
pub const POLICY_CODE: &str = "backend.abilities";
/// The preferences code of a person's own rules.
pub const OWN_POLICY_CODE: &str = "backend.my_abilities";
/// The permission to set the panel's rules.
pub const POLICY_PERMISSION: &str = "backend.manage_abilities";

/// The two pages that hold the rules: the panel's under Settings, and each
/// person's own among their preferences. `abilities` are the ones a rule
/// may name.
pub(crate) fn policy_settings(abilities: &[&Ability]) -> Vec<crate::settings::SettingsItem> {
    use crate::form::FormField;

    let mut what: Vec<(String, String)> = [
        ("read", "Everything that looks something up"),
        ("create", "Everything that adds a record"),
        ("update", "Everything that changes a record"),
        ("delete", "Everything that deletes a record"),
        ("send", "Everything that sends to someone outside"),
        ("run", "Every other change"),
    ]
    .into_iter()
    .map(|(effect, words)| (format!("effect:{effect}"), words.to_string()))
    .collect();
    let mut groups: Vec<&str> = abilities.iter().map(|a| a.group_code()).collect();
    groups.sort_unstable();
    groups.dedup();
    what.extend(
        groups
            .into_iter()
            .map(|group| (group.to_string(), format!("All of {group}"))),
    );
    what.extend(abilities.iter().map(|a| {
        (
            a.code().to_string(),
            format!("{} ({})", a.label().source(), a.code()),
        )
    }));
    let what: Vec<(&str, &str)> = what.iter().map(|(v, l)| (v.as_str(), l.as_str())).collect();
    let rules = |help: &str| {
        FormField::repeater(
            "rules",
            "Rules",
            vec![
                FormField::select("what", "For", what.clone()).required(),
                FormField::select(
                    "confirm",
                    "Allow",
                    vec![
                        ("auto", "Run without asking"),
                        ("ask", "Ask first"),
                        ("off", "Not offered"),
                    ],
                )
                .required(),
            ],
        )
        .help(help.to_string())
    };
    vec![
        crate::settings::SettingsItem::new(
            POLICY_CODE,
            "Abilities",
            vec![rules(
                "With no rule, a lookup runs, an addition or a change asks first, and a delete or a send is not offered. The rule that names an ability most exactly decides.",
            )],
        )
        .description("Set how far an assistant or an automation goes for a person.")
        .category(crate::settings::category::SYSTEM)
        .slot(laterite_core::arrange::slot::SYSTEM)
        .order(60)
        .icon("sliders-horizontal")
        .permission(POLICY_PERMISSION),
        crate::settings::SettingsItem::new(
            OWN_POLICY_CODE,
            "Acting for me",
            vec![rules(
                "A rule here tightens what the panel allows, and loosens it no further than the panel does.",
            )],
        )
        .description("Choose how far an assistant goes in your name.")
        .icon("sliders-horizontal")
        .per_operator(),
    ]
}

/// How far something acting for one person may go, ability by ability.
///
/// The panel's rule for an ability, or its effect's default, is the furthest
/// anyone goes. A person's own rule tightens that freely and loosens it only
/// up to there.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    panel: Vec<PolicyRule>,
    own: Vec<PolicyRule>,
}

impl Policy {
    /// A policy from the panel's rules and one person's.
    pub fn new(panel: Vec<PolicyRule>, own: Vec<PolicyRule>) -> Self {
        Self { panel, own }
    }

    /// The furthest anyone may go with `ability`: the panel's rule, or the
    /// default for its effect.
    pub fn ceiling(&self, ability: &Ability) -> Confirm {
        pick(&self.panel, ability).unwrap_or_else(|| Confirm::for_effect(ability.effect))
    }

    /// How far this person lets `ability` go.
    pub fn confirm(&self, ability: &Ability) -> Confirm {
        let ceiling = self.ceiling(ability);
        pick(&self.own, ability).map_or(ceiling, |own| own.min(ceiling))
    }
}

/// The rule that names `ability` most exactly: its code, then the longest
/// start of its code, then its effect.
fn pick(rules: &[PolicyRule], ability: &Ability) -> Option<Confirm> {
    let code = ability.code.as_str();
    let effect = format!("effect:{}", ability.effect.as_str());
    rules
        .iter()
        .filter_map(|rule| {
            let what = rule.what.trim();
            let weight = if what == code {
                usize::MAX
            } else if code
                .strip_prefix(what)
                .is_some_and(|rest| rest.starts_with('.'))
            {
                what.len()
            } else if what == effect {
                0
            } else {
                return None;
            };
            Some((weight, rule.confirm))
        })
        // Among rules of one weight the tighter wins, so a repeated row
        // never loosens.
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, confirm)| confirm)
}

/// The abilities of this panel, as one person may use them. Reached through
/// [`RouteCtx::abilities`].
pub struct Abilities<'a> {
    ctx: &'a RouteCtx,
}

impl<'a> Abilities<'a> {
    pub(crate) fn new(ctx: &'a RouteCtx) -> Self {
        Self { ctx }
    }

    /// Every ability `user` may run: the framework's own over records and
    /// over screens, where one is open to them, then each module's in the
    /// order they registered.
    pub fn for_user(&self, user: &AuthenticatedUser) -> Vec<&'a Ability> {
        let records: &'static [Ability] = if crate::records::any_for(self.ctx, user) {
            &crate::records::ABILITIES
        } else {
            &[]
        };
        let screens: &'static [Ability] = if crate::surface::any_for(self.ctx, user) {
            &crate::surface::ABILITIES
        } else {
            &[]
        };
        records
            .iter()
            .chain(screens.iter())
            .chain(
                self.ctx
                    .plugin_defined()
                    .items::<Ability>()
                    .into_iter()
                    .filter(|a| a.allowed(user)),
            )
            .collect()
    }

    /// The ability `code`, when `user` may run it.
    pub fn get(&self, user: &AuthenticatedUser, code: &str) -> Option<&'a Ability> {
        self.for_user(user).into_iter().find(|a| a.code() == code)
    }

    /// The abilities `user` may run that match `words`, best first, at most
    /// `limit`. A word matches a code, a label, a description or a group; no
    /// words answers the first `limit`.
    pub fn search(&self, user: &AuthenticatedUser, words: &str, limit: usize) -> Vec<&'a Ability> {
        let terms: Vec<String> = words
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 1)
            .map(str::to_lowercase)
            .collect();
        let all = self.for_user(user);
        if terms.is_empty() {
            return all.into_iter().take(limit).collect();
        }
        let mut scored: Vec<(usize, usize, &Ability)> = all
            .into_iter()
            .enumerate()
            .filter_map(|(at, ability)| {
                let name = format!(
                    "{} {}",
                    ability.code().replace(['.', '_'], " "),
                    ability.label().source().to_lowercase()
                );
                let rest = format!(
                    "{} {}",
                    ability
                        .description()
                        .map(|t| t.source().to_lowercase())
                        .unwrap_or_default(),
                    ability.group_code()
                );
                let score: usize = terms
                    .iter()
                    .map(
                        |t| match (name.contains(t.as_str()), rest.contains(t.as_str())) {
                            (true, _) => 3,
                            (false, true) => 1,
                            _ => 0,
                        },
                    )
                    .sum();
                (score > 0).then_some((score, at, ability))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().take(limit).map(|(_, _, a)| a).collect()
    }

    /// The policy for `user`: the panel's rules and their own.
    ///
    /// A rule that cannot be read fails the call: a caller that cannot learn
    /// the policy runs nothing.
    pub async fn policy(&self, user: &AuthenticatedUser) -> Result<Policy, AbilityError> {
        let db = self.ctx.db();
        let panel: PanelPolicy = crate::settings::store::load(db)
            .await
            .map_err(|e| AbilityError::Failed(format!("ability policy: {e}")))?;
        let own: OwnPolicy = crate::settings::store::load_for(db, user.user.id)
            .await
            .map_err(|e| AbilityError::Failed(format!("ability policy: {e}")))?;
        Ok(Policy::new(panel.rules, own.rules))
    }

    /// What something acting for `user` may use, each with how far it may
    /// go: [`Abilities::for_user`] without what the policy turns off.
    pub async fn offered(
        &self,
        user: &AuthenticatedUser,
    ) -> Result<Vec<(&'a Ability, Confirm)>, AbilityError> {
        let policy = self.policy(user).await?;
        Ok(self
            .for_user(user)
            .into_iter()
            .map(|ability| (ability, policy.confirm(ability)))
            .filter(|(_, confirm)| *confirm != Confirm::Off)
            .collect())
    }

    /// How far something acting for `user` may go with `code`. Ask before a
    /// run: on [`Confirm::Ask`] show the person the preview and run when
    /// they agree.
    pub async fn confirm(
        &self,
        user: &AuthenticatedUser,
        code: &str,
    ) -> Result<Confirm, AbilityError> {
        let ability = self
            .get(user, code)
            .ok_or_else(|| AbilityError::Unknown(code.to_string()))?;
        match self.policy(user).await?.confirm(ability) {
            Confirm::Off => Err(AbilityError::Unknown(code.to_string())),
            confirm => Ok(confirm),
        }
    }

    /// The policy's answer for a call through `channel`; a person acting
    /// themselves is not asked.
    async fn through(
        &self,
        user: &AuthenticatedUser,
        code: &str,
        channel: Option<&str>,
    ) -> Result<Option<Confirm>, AbilityError> {
        match channel {
            Some(_) => self.confirm(user, code).await.map(Some),
            None => Ok(None),
        }
    }

    /// What running `code` with `input` would do, with nothing changed.
    /// `channel` names what is asking for the person (`acme.assistant`);
    /// `None` when they ask themselves.
    pub async fn preview(
        &self,
        user: &AuthenticatedUser,
        code: &str,
        input: &Value,
        channel: Option<&str>,
    ) -> Result<Preview, AbilityError> {
        let ability = self.checked(user, code, input)?;
        self.through(user, code, channel).await?;
        let cx = self.cx(user, channel);
        ability.handler.preview(&cx, input).await.inspect_err(|e| {
            if let AbilityError::Failed(message) = e {
                tracing::error!(ability = code, error = %message, "an ability's preview failed");
            }
        })
    }

    /// Runs `code` with `input` as `user`, and records it.
    ///
    /// The ability must be one `user` may run and `input` must fit its
    /// schema; either failing runs nothing. Through a `channel`, an ability
    /// the [`Policy`] turns off answers as one that does not exist; ask
    /// [`Abilities::confirm`] first to learn whether to show the person
    /// the preview. `channel` names what is acting
    /// for the person (`acme.assistant`), and reaches the audit entry and
    /// every record the handler saves through [`AbilityCx::actor`].
    ///
    /// An ability that changes anything writes one `ability.run` audit entry
    /// when it succeeds, beside whatever entries its own writes produce. A
    /// read writes none.
    pub async fn run(
        &self,
        user: &AuthenticatedUser,
        code: &str,
        input: Value,
        channel: Option<&str>,
    ) -> Result<Outcome, AbilityError> {
        let ability = self.checked(user, code, &input)?;
        let confirm = self.through(user, code, channel).await?;
        let cx = self.cx(user, channel);
        let logged = ability.effect.changes().then(|| redacted(ability, &input));
        let outcome = ability.handler.run(&cx, input).await.inspect_err(|e| {
            if let AbilityError::Failed(message) = e {
                tracing::error!(ability = code, error = %message, "an ability failed");
            }
        })?;
        if let Some(input) = logged {
            let mut detail = json!({
                "effect": ability.effect.as_str(),
                "input": input,
                "result": outcome.message.source(),
            });
            if let Some(confirm) = confirm {
                detail["confirm"] = json!(confirm.as_str());
            }
            let detail = detail.to_string();
            let entry = AuditEntry::new(cx.actor(), "ability.run")
                .target("ability", code, Some(ability.label.source()))
                .detail(&detail);
            if let Err(e) = self.ctx.auth_service().record_audit(entry).await {
                tracing::error!(ability = code, error = %e, "failed to write audit log entry");
            }
        }
        Ok(outcome)
    }

    fn cx<'u>(&self, user: &'u AuthenticatedUser, channel: Option<&str>) -> AbilityCx<'u>
    where
        'a: 'u,
    {
        let actor = Actor::from(user);
        AbilityCx {
            ctx: self.ctx,
            user,
            actor: match channel {
                Some(channel) => actor.through(channel),
                None => actor,
            },
        }
    }

    /// The ability, once the person may run it and the input fits.
    fn checked(
        &self,
        user: &AuthenticatedUser,
        code: &str,
        input: &Value,
    ) -> Result<&'a Ability, AbilityError> {
        let ability = self
            .get(user, code)
            .ok_or_else(|| AbilityError::Unknown(code.to_string()))?;
        let validator = jsonschema::validator_for(&ability.input)
            .map_err(|e| AbilityError::Failed(format!("input schema of `{code}`: {e}")))?;
        let faults: Vec<String> = validator
            .iter_errors(input)
            .map(|e| {
                let at = e.instance_path().to_string();
                if at.is_empty() {
                    e.to_string()
                } else {
                    format!("{at}: {e}")
                }
            })
            .collect();
        if faults.is_empty() {
            Ok(ability)
        } else {
            Err(AbilityError::Invalid(faults))
        }
    }
}

/// The input as the audit entry keeps it: sensitive fields blanked.
fn redacted(ability: &Ability, input: &Value) -> Value {
    let mut kept = input.clone();
    if let Some(object) = kept.as_object_mut() {
        for field in &ability.sensitive {
            if let Some(value) = object.get_mut(field) {
                *value = Value::String("[kept out]".to_string());
            }
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterite_core::{t, ModuleId};

    struct Nothing;

    #[async_trait]
    impl AbilityHandler for Nothing {
        async fn run(&self, _cx: &AbilityCx<'_>, _input: Value) -> Result<Outcome, AbilityError> {
            Ok(Outcome::new(t!("Done.")))
        }
    }

    fn checked(abilities: Vec<Ability>) -> Result<(), String> {
        let mut registry = Registry::new();
        registry.set_owner(ModuleId::new("acme.crm"));
        for ability in abilities {
            registry.add(ability);
        }
        check(&registry)
    }

    fn ability(code: &str) -> Ability {
        Ability::new(code, t!("Do it"), Arc::new(Nothing))
    }

    #[test]
    fn a_guarded_change_and_an_open_read_pass() {
        assert_eq!(
            checked(vec![
                ability("acme.crm.add_lead").permission("acme.crm.leads.create"),
                ability("acme.crm.count_leads").effect(Effect::Read),
            ]),
            Ok(())
        );
    }

    #[test]
    fn a_change_nobody_guards_is_refused() {
        let refused = checked(vec![ability("acme.crm.add_lead")]).unwrap_err();
        assert!(refused.contains("names no permission"), "{refused}");
    }

    #[test]
    fn a_malformed_or_repeated_code_is_refused() {
        for code in ["add_lead", "Acme.crm.add", "acme..add", "acme.crm.add-lead"] {
            let refused = checked(vec![ability(code).effect(Effect::Read)]).unwrap_err();
            assert!(refused.contains("dotted lowercase"), "{code}: {refused}");
        }
        let twice = checked(vec![
            ability("acme.crm.count").effect(Effect::Read),
            ability("acme.crm.count").effect(Effect::Read),
        ])
        .unwrap_err();
        assert!(twice.contains("registered twice"), "{twice}");
    }

    #[test]
    fn an_input_that_is_no_object_schema_is_refused() {
        let refused = checked(vec![ability("acme.crm.count")
            .effect(Effect::Read)
            .input_schema(json!({ "type": "string" }))])
        .unwrap_err();
        assert!(refused.contains("must be an object"), "{refused}");
    }

    #[test]
    fn a_typed_input_declares_its_fields_inline() {
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Inner {
            city: String,
        }
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Input {
            name: String,
            address: Option<Inner>,
        }
        let ability = ability("acme.crm.count").input::<Input>();
        let schema = ability.input_schema_value();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["name"]));
        // Nothing is referenced: a caller reads the whole shape in one place.
        assert!(!schema.to_string().contains("$ref"), "{schema}");
    }
}
