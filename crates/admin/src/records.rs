//! The abilities every resource gives for nothing: finding its records,
//! reading one, drafting a new one or a change, and deleting one.
//!
//! A resource descriptor already states the table, the permission, the
//! searchable columns and each field's type, label and rules, so nothing more
//! is written for these to exist. `abilities:` on the resource says what a
//! caller is told about it and switches parts off; see [`RecordAbilities`].
//!
//! They are a few fixed abilities over a list of kinds of record, never one per
//! resource: a panel with eighty resources offers a caller the same few.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use laterite_auth::AuthenticatedUser;
use laterite_core::serde_json::{json, Map, Value};
use laterite_core::strata::async_trait;
use laterite_core::validation::Rule;
use laterite_core::{t, QueryHook, Text};
use serde::{Deserialize, Serialize};

use crate::ability::{
    Ability, AbilityCx, AbilityError, AbilityHandler, Draft, Effect, Link, Outcome, Preview,
};
use crate::form::PreparedForm;
use crate::list::{ListConfig, ListQuery};
use crate::persist::SaveError;
use crate::routes::RouteCtx;
use crate::ActionPermissions;

/// What a resource lets a caller of abilities do with its records:
/// `abilities:` in a descriptor file, either `false` or a map of these keys.
///
/// A resource offers find, read, create and update unless told otherwise,
/// each still asking for the permission its screen asks for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct RecordAbilities {
    /// Whether the resource is offered at all.
    pub enabled: bool,
    /// What the records are, for a caller choosing where to look: "Blog
    /// posts. A post has a title, a body in markdown and a publish date."
    pub describe: Option<Text>,
    /// Whether a new record may be drafted.
    pub create: bool,
    /// Whether a change to a record may be drafted.
    pub update: bool,
    /// Whether a record may be deleted, where the list deletes. The panel's
    /// policy keeps deleting off until an administrator allows it.
    pub delete: bool,
    /// Fields and columns kept from a caller, by name.
    pub hide: Vec<String>,
    /// The records carry text written by people outside the panel (a
    /// submission, a comment), which a caller must not take as an instruction.
    pub untrusted: bool,
}

impl Default for RecordAbilities {
    fn default() -> Self {
        Self {
            enabled: true,
            describe: None,
            create: true,
            update: true,
            delete: true,
            hide: Vec::new(),
            untrusted: false,
        }
    }
}

impl RecordAbilities {
    /// The default: find, read, create and update, nothing hidden.
    pub fn new() -> Self {
        Self::default()
    }

    /// A resource no ability reaches.
    pub fn off() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Says what the records are.
    pub fn describe(mut self, text: Text) -> Self {
        self.describe = Some(text);
        self
    }

    /// Records are found and read, never drafted or deleted.
    pub fn read_only(mut self) -> Self {
        self.create = false;
        self.update = false;
        self.delete = false;
        self
    }

    /// Records are never deleted through an ability.
    pub fn keep(mut self) -> Self {
        self.delete = false;
        self
    }

    /// Keeps the named fields and columns from a caller.
    pub fn hide<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.hide = fields.into_iter().map(Into::into).collect();
        self
    }

    /// Marks the records as carrying text written by outsiders.
    pub fn untrusted(mut self) -> Self {
        self.untrusted = true;
        self
    }
}

/// `abilities:` as a file writes it: a flag, or the keys.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RecordAbilitiesKey {
    Flag(bool),
    Config(RecordAbilities),
}

impl From<RecordAbilitiesKey> for RecordAbilities {
    fn from(key: RecordAbilitiesKey) -> Self {
        match key {
            RecordAbilitiesKey::Flag(true) => RecordAbilities::new(),
            RecordAbilitiesKey::Flag(false) => RecordAbilities::off(),
            RecordAbilitiesKey::Config(config) => config,
        }
    }
}

/// One resource as the record abilities see it.
pub(crate) struct RecordKind {
    /// The resource's path as authored (`/posts`), which names the kind.
    pub key: String,
    /// The same path under the admin mount, for links.
    pub href: String,
    pub title: Text,
    pub list: ListConfig,
    pub form: Option<Arc<PreparedForm>>,
    /// The list's own delete path, when the list deletes.
    pub delete: Option<Arc<crate::bulk::BulkContext>>,
    pub permission: Option<String>,
    pub permissions: ActionPermissions,
    pub hooks: Vec<Arc<dyn QueryHook>>,
    pub config: RecordAbilities,
}

/// Field types whose value never leaves the server and is never set by a
/// caller of abilities.
const SECRET_TYPES: [&str; 2] = ["password", "sensitive"];

/// The most rows one find answers.
const MAX_ROWS: i64 = 25;

impl RecordKind {
    pub(crate) fn sees(&self, user: &AuthenticatedUser) -> bool {
        self.config.enabled
            && self
                .permission
                .as_deref()
                .is_none_or(|p| user.permissions.allows(p))
    }

    fn may(&self, user: &AuthenticatedUser, action: &Option<String>) -> bool {
        self.sees(user) && action.as_deref().is_none_or(|p| user.permissions.allows(p))
    }

    fn creates(&self, user: &AuthenticatedUser) -> bool {
        self.form.is_some()
            && self.config.create
            && self.list.creatable
            && self.may(user, &self.permissions.create)
    }

    fn updates(&self, user: &AuthenticatedUser) -> bool {
        self.form.is_some() && self.config.update && self.may(user, &self.permissions.update)
    }

    fn deletes(&self, user: &AuthenticatedUser) -> bool {
        self.form.is_some()
            && self.delete.is_some()
            && self.config.delete
            && self.may(user, &self.permissions.delete)
    }

    fn hidden(&self, name: &str) -> bool {
        self.config.hide.iter().any(|h| h == name)
    }

    /// The form's fields a caller may read and set.
    fn fields(&self) -> Vec<&crate::form::FormField> {
        self.form
            .iter()
            .flat_map(|form| form.config.fields.iter())
            .filter(|f| {
                !f.hidden && !self.hidden(&f.name) && !SECRET_TYPES.contains(&&*f.field_type)
            })
            .collect()
    }

    fn edit_href(&self, id: &str) -> String {
        format!("{}/{id}/edit", self.href)
    }

    fn summary(&self, user: &AuthenticatedUser) -> Value {
        let mut can = vec!["find"];
        if self.form.is_some() {
            can.push("read");
        }
        if self.creates(user) {
            can.push("create");
        }
        if self.updates(user) {
            can.push("update");
        }
        if self.deletes(user) {
            can.push("delete");
        }
        json!({
            "resource": self.key,
            "title": self.title.source(),
            "describe": self.config.describe.as_ref().map(Text::source),
            "can": can,
        })
    }

    fn detail(&self, user: &AuthenticatedUser) -> Value {
        let mut detail = self.summary(user);
        let fields: Vec<Value> = self
            .fields()
            .into_iter()
            .map(|f| {
                let mut field = Map::new();
                field.insert("name".into(), json!(f.name));
                field.insert("label".into(), json!(f.label.source()));
                field.insert("type".into(), json!(f.field_type));
                if f.rules.iter().any(|r| matches!(r, Rule::Required)) {
                    field.insert("required".into(), json!(true));
                }
                if let Some(help) = &f.help {
                    field.insert("help".into(), json!(help.source()));
                }
                if !f.options.is_null() {
                    field.insert("options".into(), f.options.clone());
                }
                Value::Object(field)
            })
            .collect();
        let columns: Vec<&str> = self.columns(user).map(|(_, c)| c.field.as_str()).collect();
        let filters: Vec<Value> = crate::list::allowed_filters(&self.list, user)
            .into_iter()
            .map(|f| {
                let mut filter = Map::new();
                filter.insert("name".into(), json!(f.field));
                filter.insert("label".into(), json!(f.label.source()));
                filter.insert("type".into(), json!(f.filter_type));
                match f.filter_type.as_str() {
                    "boolean" => {
                        filter.insert("values".into(), json!(["1", "0"]));
                    }
                    "select" => {
                        let values: Vec<String> =
                            f.choices().into_iter().map(|o| o.value).collect();
                        filter.insert("values".into(), json!(values));
                    }
                    "number" => {
                        filter.insert("match".into(), json!(f.matching()));
                    }
                    "date" => {
                        filter.insert("format".into(), json!("YYYY-MM-DD"));
                    }
                    _ => {}
                }
                Value::Object(filter)
            })
            .collect();
        let sorts: Vec<&str> = self
            .columns(user)
            .filter(|(_, c)| c.sortable)
            .map(|(_, c)| c.field.as_str())
            .collect();
        detail["fields"] = json!(fields);
        detail["columns"] = json!(columns);
        detail["filters"] = json!(filters);
        detail["sorts"] = json!(sorts);
        detail
    }

    /// The list's query parameters for what a caller asked: the search, the
    /// filters by name and the order. A filter the list does not offer this
    /// person, a value its type refuses and a column it does not sort by are
    /// each named, so the caller can ask again.
    fn asked(
        &self,
        user: &AuthenticatedUser,
        input: &Value,
    ) -> Result<Vec<(String, String)>, AbilityError> {
        let mut params = Vec::new();
        let search = text(input, "search").trim();
        if !search.is_empty() {
            params.push(("q".to_string(), search.to_string()));
        }
        let offered = crate::list::allowed_filters(&self.list, user);
        let mut faults = Vec::new();
        for (name, value) in input
            .get("filters")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let value = match value {
                Value::String(text) => text.trim().to_string(),
                Value::Bool(yes) => if *yes { "1" } else { "0" }.to_string(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            let Some(filter) = offered.iter().find(|f| f.field == *name) else {
                faults.push(format!("filters/{name}: this kind has no such filter"));
                continue;
            };
            let param = filter.param();
            let one = HashMap::from([(param.clone(), value.clone())]);
            let accepted = crate::list::resolve_filters(&self.list, &one)
                .iter()
                .any(|a| a.is_for(filter));
            if !accepted {
                faults.push(format!(
                    "filters/{name}: `{value}` is not a value a {} filter takes",
                    filter.filter_type
                ));
                continue;
            }
            params.push((param, value));
        }
        let sort = text(input, "sort");
        if !sort.is_empty() {
            if self
                .columns(user)
                .any(|(_, c)| c.field == sort && c.sortable)
            {
                params.push(("sort".to_string(), sort.to_string()));
                let dir = match text(input, "dir") {
                    "desc" => "desc",
                    _ => "asc",
                };
                params.push(("dir".to_string(), dir.to_string()));
            } else {
                faults.push(format!("sort: this kind does not sort by `{sort}`"));
            }
        }
        if faults.is_empty() {
            Ok(params)
        } else {
            Err(AbilityError::Invalid(faults))
        }
    }

    /// The list as a surface names it in `backend.screens.list`.
    pub(crate) fn surface_summary(&self) -> Value {
        json!({
            "screen": self.key,
            "label": self.title.source(),
            "describe": self.config.describe.as_ref().map(Text::source),
        })
    }

    /// What a surface calls the control for `filter`: its name, set apart
    /// from the list's own three controls.
    fn control_name(filter: &crate::list::ListFilter) -> String {
        match filter.field.as_str() {
            "search" | "sort" | "dir" => format!("filter_{}", filter.field),
            name => name.to_string(),
        }
    }

    /// The list as a surface: its search, its filters and its order as
    /// controls, shown or read.
    pub(crate) fn surface_part(&self, user: &AuthenticatedUser) -> crate::surface::Part {
        use crate::surface::{Control, Part, READ, SHOW};

        let mut part = Part::new("list", self.title.clone())
            .act(SHOW)
            .act(READ)
            .control(Control::new("search", "Search", "text").describe("Words to match."));
        if let Some(describe) = &self.config.describe {
            part = part.describe(describe.clone());
        }
        for filter in crate::list::allowed_filters(&self.list, user) {
            let mut control = Control::new(
                Self::control_name(filter),
                filter.label.clone(),
                filter.filter_type.clone(),
            );
            control = match filter.filter_type.as_str() {
                "boolean" => control.values(["1", "0"]),
                "select" => control.values(filter.choices().into_iter().map(|o| o.value)),
                "number" => match filter.matching() {
                    "at_least" => control.describe("Keeps rows at or above this whole number."),
                    "at_most" => control.describe("Keeps rows at or below this whole number."),
                    _ => control.describe("Keeps rows with exactly this whole number."),
                },
                "date" => control.describe("One day, as YYYY-MM-DD."),
                _ => control,
            };
            if let Some(default) = &filter.default_value {
                control = control.value(default.clone());
            }
            part = part.control(control);
        }
        let sorts: Vec<&str> = self
            .columns(user)
            .filter(|(_, c)| c.sortable)
            .map(|(_, c)| c.field.as_str())
            .collect();
        if !sorts.is_empty() {
            part = part
                .control(
                    Control::new("sort", "Sort by", "select")
                        .values(sorts)
                        .value(self.list.order_by.clone()),
                )
                .control(Control::new("dir", "Direction", "select").values(["asc", "desc"]));
        }
        part
    }

    /// Shows or reads the list with the controls a request sets.
    pub(crate) async fn surface_act(
        &self,
        cx: &AbilityCx<'_>,
        request: &crate::surface::Request,
    ) -> Result<Outcome, AbilityError> {
        if !request.part.is_empty() {
            return Err(AbilityError::invalid(format!(
                "part: `{}` is not a part of this screen",
                request.part.join("/")
            )));
        }
        let mut input = Map::new();
        input.insert("resource".into(), json!(self.key));
        let mut filters = Map::new();
        let offered = crate::list::allowed_filters(&self.list, cx.user());
        let mut faults = Vec::new();
        for (name, value) in &request.set {
            match name.as_str() {
                "search" | "sort" | "dir" => {
                    input.insert(name.clone(), value.clone());
                }
                _ => match offered.iter().find(|f| Self::control_name(f) == *name) {
                    Some(filter) => {
                        filters.insert(filter.field.clone(), value.clone());
                    }
                    None => faults.push(format!("set/{name}: this part has no such control")),
                },
            }
        }
        if !faults.is_empty() {
            return Err(AbilityError::Invalid(faults));
        }
        input.insert("filters".into(), Value::Object(filters));
        let input = Value::Object(input);
        match request.act.as_str() {
            crate::surface::SHOW => Open.run(cx, input).await,
            crate::surface::READ => Find.run(cx, input).await,
            other => Err(AbilityError::invalid(format!(
                "act: this part does not `{other}`; it offers: show, read"
            ))),
        }
    }

    /// The list's columns a caller is shown, with their place in a row.
    fn columns<'a>(
        &'a self,
        user: &'a AuthenticatedUser,
    ) -> impl Iterator<Item = (usize, &'a crate::list::ListColumn)> {
        self.list.columns.iter().enumerate().filter(move |(_, c)| {
            !self.hidden(&c.field)
                && c.permission
                    .as_deref()
                    .is_none_or(|p| user.permissions.allows(p))
        })
    }
}

/// Whether any kind of record is open to `user`: without one the record
/// abilities are not offered.
pub(crate) fn any_for(ctx: &RouteCtx, user: &AuthenticatedUser) -> bool {
    ctx.records().iter().any(|k| k.sees(user))
}

/// The kind `resource` names, when `user` may see it. One answer for a kind
/// that does not exist and one the person may not see.
fn kind<'a>(cx: &AbilityCx<'a>, resource: &str) -> Result<&'a RecordKind, AbilityError> {
    cx.ctx()
        .records()
        .iter()
        .find(|k| k.key == resource && k.sees(cx.user()))
        .ok_or_else(|| {
            AbilityError::invalid(format!(
                "no kind of record is named `{resource}`: list them with backend.records.kinds"
            ))
        })
}

fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn record_id(input: &Value) -> Result<i64, AbilityError> {
    match input.get("id") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
    .ok_or_else(|| AbilityError::invalid("id: a record is named by its whole-number id"))
}

/// The submitted values, each as the form's control would send it. A name the
/// kind does not offer is refused, so a hidden field is never written.
fn values(kind: &RecordKind, input: &Value) -> Result<HashMap<String, String>, AbilityError> {
    let offered = kind.fields();
    let mut out = HashMap::new();
    let mut faults = Vec::new();
    for (name, value) in input
        .get("values")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if !offered.iter().any(|f| &f.name == name) {
            faults.push(format!(
                "values/{name}: this kind of record has no such field"
            ));
            continue;
        }
        out.insert(
            name.clone(),
            match value {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                Value::Bool(b) => if *b { "1" } else { "" }.to_string(),
                other => other.to_string(),
            },
        );
    }
    if faults.is_empty() {
        Ok(out)
    } else {
        Err(AbilityError::Invalid(faults))
    }
}

/// A refused save as an ability answers it. Messages against a field the
/// caller cannot set are left for the person, who fills it on the form.
fn refused(kind: &RecordKind, error: SaveError) -> Result<(), AbilityError> {
    match error {
        SaveError::Invalid(bag) => {
            let offered = kind.fields();
            let faults: Vec<String> = bag
                .iter()
                .filter(|(field, _)| offered.iter().any(|f| f.name == *field))
                .flat_map(|(field, messages)| {
                    messages
                        .iter()
                        .map(move |m| format!("values/{field}: {}", m.source()))
                })
                .collect();
            if faults.is_empty() {
                Ok(())
            } else {
                Err(AbilityError::Invalid(faults))
            }
        }
        SaveError::Failed(message) => Err(AbilityError::Failed(message)),
    }
}

fn save_error(error: SaveError) -> AbilityError {
    match error {
        SaveError::Invalid(bag) => AbilityError::Invalid(
            bag.iter()
                .flat_map(|(field, messages)| {
                    messages
                        .iter()
                        .map(move |m| format!("values/{field}: {}", m.source()))
                })
                .collect(),
        ),
        SaveError::Failed(message) => AbilityError::Failed(message),
    }
}

/// The record as its form holds it, through the entity's scopes, with the
/// fields a caller is not shown left in: the caller of this decides.
async fn stored(
    cx: &AbilityCx<'_>,
    kind: &RecordKind,
    id: i64,
) -> Result<HashMap<String, String>, AbilityError> {
    let form = kind
        .form
        .as_ref()
        .ok_or_else(|| AbilityError::Refused(t!("These records are listed, never opened.")))?;
    crate::form::load_values(cx.db(), form, &id.to_string(), &kind.hooks)
        .await
        .map_err(|e| AbilityError::Failed(e.to_string()))?
        .ok_or_else(|| AbilityError::Refused(t!("No record has that id.")))
}

struct Kinds;

#[async_trait]
impl AbilityHandler for Kinds {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let resource = text(&input, "resource");
        let data = if resource.is_empty() {
            json!(cx
                .ctx()
                .records()
                .iter()
                .filter(|k| k.sees(cx.user()))
                .map(|k| k.summary(cx.user()))
                .collect::<Vec<_>>())
        } else {
            kind(cx, resource)?.detail(cx.user())
        };
        Ok(Outcome::new(t!("The kinds of record.")).data(data))
    }
}

struct Find;

#[async_trait]
impl AbilityHandler for Find {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let kind = kind(cx, text(&input, "resource"))?;
        let limit = input
            .get("limit")
            .and_then(Value::as_i64)
            .unwrap_or(10)
            .clamp(1, MAX_ROWS);
        let params: HashMap<String, String> = kind.asked(cx.user(), &input)?.into_iter().collect();
        // Only what was asked narrows a find: a list's default filter is for
        // the screen.
        let mut unset = kind.list.clone();
        for filter in &mut unset.filters {
            filter.default_value = None;
        }
        let filters = crate::list::resolve_filters(&unset, &params);
        let (order_by, order_dir) = crate::list::resolve_sort(
            &kind.list,
            params.get("sort").map(String::as_str),
            params.get("dir").map(String::as_str),
        );
        let page = crate::list::query(
            cx.db(),
            &kind.list,
            &ListQuery {
                offset: 0,
                order_by: &order_by,
                order_dir,
                q: text(&input, "search").trim(),
                filters: &filters,
                per_page: Some(limit),
                children_of: None,
                hooks: &kind.hooks,
                trashed: false,
            },
        )
        .await
        .map_err(|e| AbilityError::Failed(e.to_string()))?;
        let columns: Vec<_> = kind.columns(cx.user()).collect();
        let mut outcome = Outcome::new(t!(
            "Found {n} of {total}.",
            n = page.rows.len() as i64,
            total = page.total
        ));
        let mut rows = Vec::with_capacity(page.rows.len());
        for row in &page.rows {
            let mut out = Map::new();
            out.insert("id".into(), json!(row.id));
            for (at, column) in &columns {
                out.insert(column.field.clone(), json!(row.cells[*at]));
            }
            rows.push(Value::Object(out));
            if kind.form.is_some() {
                let label = columns
                    .first()
                    .map(|(at, _)| row.cells[*at].clone())
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| row.id.clone());
                outcome = outcome.link(Link::new(label, kind.edit_href(&row.id)));
            }
        }
        outcome = outcome.data(json!({ "total": page.total, "rows": rows }));
        Ok(if kind.config.untrusted {
            outcome.untrusted()
        } else {
            outcome
        })
    }
}

struct Open;

#[async_trait]
impl AbilityHandler for Open {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let kind = kind(cx, text(&input, "resource"))?;
        let params = kind.asked(cx.user(), &input)?;
        let mut href = kind.href.clone();
        for (at, (name, value)) in params.iter().enumerate() {
            href.push(if at == 0 { '?' } else { '&' });
            href.push_str(&crate::list::urlencode(name));
            href.push('=');
            href.push_str(&crate::list::urlencode(value));
        }
        let title = kind.title.source().to_string();
        Ok(Outcome::new(t!("Opened {kind}.", kind = title.clone()))
            .link(Link::new(title, href.clone()))
            .go(href))
    }
}

struct Read;

#[async_trait]
impl AbilityHandler for Read {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let kind = kind(cx, text(&input, "resource"))?;
        let id = record_id(&input)?;
        let stored = stored(cx, kind, id).await?;
        let shown: Map<String, Value> = kind
            .fields()
            .into_iter()
            .filter_map(|f| stored.get(&f.name).map(|v| (f.name.clone(), json!(v))))
            .collect();
        let outcome = Outcome::new(t!("The record."))
            .link(Link::new(
                kind.title.source().to_string(),
                kind.edit_href(&id.to_string()),
            ))
            .data(json!({ "id": id, "values": shown }));
        Ok(if kind.config.untrusted {
            outcome.untrusted()
        } else {
            outcome
        })
    }
}

/// A new record, or a change to one: `id` in the input decides.
struct Write {
    update: bool,
}

impl Write {
    /// The kind, the record's id on an update, and every value the save would
    /// submit: the caller's laid over what the record holds.
    async fn prepared<'a>(
        &self,
        cx: &AbilityCx<'a>,
        input: &Value,
    ) -> Result<(&'a RecordKind, Option<i64>, HashMap<String, String>), AbilityError> {
        let kind = kind(cx, text(input, "resource"))?;
        let allowed = if self.update {
            kind.updates(cx.user())
        } else {
            kind.creates(cx.user())
        };
        if !allowed {
            return Err(AbilityError::Refused(if self.update {
                t!("You may not change these records.")
            } else {
                t!("You may not add these records.")
            }));
        }
        let supplied = values(kind, input)?;
        if !self.update {
            return Ok((kind, None, supplied));
        }
        let id = record_id(input)?;
        let mut merged = stored(cx, kind, id).await?;
        merged.extend(supplied);
        Ok((kind, Some(id), merged))
    }
}

#[async_trait]
impl AbilityHandler for Write {
    async fn preview(&self, cx: &AbilityCx<'_>, input: &Value) -> Result<Preview, AbilityError> {
        let (kind, id, data) = self.prepared(cx, input).await?;
        let handle = cx
            .ctx()
            .form(&kind.key)
            .ok_or_else(|| AbilityError::Failed(format!("no form at `{}`", kind.key)))?;
        if let Err(e) = handle.check(id, data.clone()).await {
            refused(kind, e)?;
        }
        let title = kind.title.source().to_string();
        let summary = match id {
            Some(_) => t!("Changes a record in {kind}.", kind = title),
            None => t!("Adds a record to {kind}.", kind = title),
        };
        let mut preview = Preview::new(summary).draft(Draft::new(kind.key.clone(), id, data));
        if let Some(id) = id {
            preview = preview.record(Link::new(
                kind.title.source().to_string(),
                kind.edit_href(&id.to_string()),
            ));
        }
        Ok(preview)
    }

    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let (kind, id, data) = self.prepared(cx, &input).await?;
        let handle = cx
            .ctx()
            .form(&kind.key)
            .ok_or_else(|| AbilityError::Failed(format!("no form at `{}`", kind.key)))?;
        let (id, message) = match id {
            Some(id) => {
                handle
                    .update(id, data, cx.actor())
                    .await
                    .map_err(save_error)?;
                (id, t!("Saved the change."))
            }
            None => (
                handle.create(data, cx.actor()).await.map_err(save_error)?,
                t!("Added the record."),
            ),
        };
        Ok(Outcome::new(message)
            .link(Link::new(
                kind.title.source().to_string(),
                kind.edit_href(&id.to_string()),
            ))
            .data(json!({ "id": id })))
    }
}

struct Delete;

impl Delete {
    /// The kind and the id, once the person may delete it and it is there
    /// for them to see.
    async fn prepared<'a>(
        cx: &AbilityCx<'a>,
        input: &Value,
    ) -> Result<(&'a RecordKind, i64), AbilityError> {
        let kind = kind(cx, text(input, "resource"))?;
        if !kind.deletes(cx.user()) {
            return Err(AbilityError::Refused(t!(
                "You may not delete these records."
            )));
        }
        let id = record_id(input)?;
        stored(cx, kind, id).await?;
        Ok((kind, id))
    }
}

#[async_trait]
impl AbilityHandler for Delete {
    async fn preview(&self, cx: &AbilityCx<'_>, input: &Value) -> Result<Preview, AbilityError> {
        let (kind, id) = Self::prepared(cx, input).await?;
        let title = kind.title.source().to_string();
        let soft = kind.delete.as_ref().is_some_and(|d| d.soft_delete);
        let summary = if soft {
            t!(
                "Moves record {id} in {kind} to the trash.",
                id = id,
                kind = title.clone()
            )
        } else {
            t!(
                "Deletes record {id} in {kind} for good.",
                id = id,
                kind = title.clone()
            )
        };
        Ok(Preview::new(summary)
            .record(Link::new(title, kind.edit_href(&id.to_string())))
            .count(1))
    }

    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let (kind, id) = Self::prepared(cx, &input).await?;
        let Some(delete) = kind.delete.as_ref() else {
            return Err(AbilityError::Refused(t!(
                "You may not delete these records."
            )));
        };
        let mut outcome = crate::bulk::remove(
            cx.db(),
            cx.ctx().cache(),
            delete,
            &id.to_string(),
            cx.actor(),
        )
        .await;
        if let Some(reason) = outcome.refused.pop() {
            return Err(AbilityError::Refused(reason));
        }
        if outcome.deleted == 0 {
            return Err(AbilityError::Failed(format!(
                "deleting {} {id} failed",
                kind.key
            )));
        }
        let message = if delete.soft_delete {
            t!("Moved the record to the trash.")
        } else {
            t!("Deleted the record.")
        };
        Ok(Outcome::new(message)
            .link(Link::new(
                kind.title.source().to_string(),
                kind.href.clone(),
            ))
            .data(json!({ "id": id })))
    }
}

fn filters_property() -> Value {
    json!({
        "type": "object",
        "description": "The kind's filters by name, as backend.records.kinds lists them, each with the value to narrow by.",
        "additionalProperties": { "type": ["string", "number", "boolean"] }
    })
}

fn sort_property() -> Value {
    json!({ "type": "string", "description": "A column the kind sorts by, from its `sorts`." })
}

fn dir_property() -> Value {
    json!({ "type": "string", "enum": ["asc", "desc"] })
}

fn resource_property() -> Value {
    json!({
        "type": "string",
        "description": "The kind of record, as backend.records.kinds names it (`/posts`)."
    })
}

fn values_property() -> Value {
    json!({
        "type": "object",
        "description": "Field values by field name, each as text. Ask backend.records.kinds for the kind's fields first.",
        "additionalProperties": { "type": ["string", "number", "boolean", "null"] }
    })
}

fn id_property() -> Value {
    json!({ "type": ["integer", "string"], "description": "The record's id." })
}

/// The abilities over records, offered wherever some kind of record is open
/// to the person. Each checks the kind's own permissions as it runs.
pub(crate) static ABILITIES: LazyLock<Vec<Ability>> = LazyLock::new(|| {
    vec![
        Ability::new(
            "backend.records.kinds",
            t!("List the kinds of record"),
            Arc::new(Kinds),
        )
        .describe(t!(
            "Lists the kinds of record this person may work with. Give a kind to get its fields and columns."
        ))
        .input_schema(json!({
            "type": "object",
            "properties": { "resource": resource_property() },
            "additionalProperties": false
        }))
        .effect(Effect::Read),
        Ability::new("backend.records.find", t!("Find records"), Arc::new(Find))
            .describe(t!(
                "Searches one kind of record by words and answers the matching rows with their ids."
            ))
            .input_schema(json!({
                "type": "object",
                "properties": {
                    "resource": resource_property(),
                    "search": { "type": "string", "description": "Words to match. Leave out for the first rows." },
                    "filters": filters_property(),
                    "sort": sort_property(),
                    "dir": dir_property(),
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_ROWS }
                },
                "required": ["resource"],
                "additionalProperties": false
            }))
            .effect(Effect::Read),
        Ability::new("backend.records.open", t!("Open a list"), Arc::new(Open))
            .describe(t!(
                "Takes the person to one kind's list with a search, filters and an order set, for them to look through. Use it when they ask to see or show records."
            ))
            .input_schema(json!({
                "type": "object",
                "properties": {
                    "resource": resource_property(),
                    "search": { "type": "string", "description": "Words to match." },
                    "filters": filters_property(),
                    "sort": sort_property(),
                    "dir": dir_property()
                },
                "required": ["resource"],
                "additionalProperties": false
            }))
            .effect(Effect::Read),
        Ability::new("backend.records.read", t!("Read a record"), Arc::new(Read))
            .describe(t!("Answers one record's field values."))
            .input_schema(json!({
                "type": "object",
                "properties": { "resource": resource_property(), "id": id_property() },
                "required": ["resource", "id"],
                "additionalProperties": false
            }))
            .effect(Effect::Read),
        Ability::new(
            "backend.records.create",
            t!("Add a record"),
            Arc::new(Write { update: false }),
        )
        .describe(t!(
            "Drafts a new record of one kind. The person reviews the filled form and saves it."
        ))
        .input_schema(json!({
            "type": "object",
            "properties": { "resource": resource_property(), "values": values_property() },
            "required": ["resource", "values"],
            "additionalProperties": false
        }))
        .effect(Effect::Create),
        Ability::new(
            "backend.records.update",
            t!("Change a record"),
            Arc::new(Write { update: true }),
        )
        .describe(t!(
            "Drafts a change to one record: give only the fields that change. The person reviews the form and saves it."
        ))
        .input_schema(json!({
            "type": "object",
            "properties": {
                "resource": resource_property(),
                "id": id_property(),
                "values": values_property()
            },
            "required": ["resource", "id", "values"],
            "additionalProperties": false
        }))
        .effect(Effect::Update),
        Ability::new(
            "backend.records.delete",
            t!("Delete a record"),
            Arc::new(Delete),
        )
        .describe(t!(
            "Deletes one record, to the trash where the kind keeps one. The person agrees first."
        ))
        .input_schema(json!({
            "type": "object",
            "properties": { "resource": resource_property(), "id": id_property() },
            "required": ["resource", "id"],
            "additionalProperties": false
        }))
        .effect(Effect::Delete),
    ]
});
