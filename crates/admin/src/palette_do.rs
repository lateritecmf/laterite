//! The palette's modes, and the mode the framework ships: Do, which runs an
//! ability by name.
//!
//! The palette asks here which modes the person has. Do answers what they
//! type with the abilities that match, then walks one through its input, a
//! card that says what it will do, and its result, with no model involved.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use laterite_auth::AuthenticatedUser;
use laterite_core::serde_json::{self, Map, Value};
use laterite_core::t;

use crate::ability::{Ability, AbilityError, Effect, Link, Outcome, Preview};
use crate::palette::PaletteMode;
use crate::routes::RouteCtx;
use crate::{render, Shell};

/// The code and the prefix of the framework's own mode.
const DO: &str = "backend.do";

/// Abilities shown for one query.
const LIMIT: usize = 8;

pub(crate) struct DoState {
    pub ctx: RouteCtx,
    /// The admin mount, for the mode's own links.
    pub admin: String,
}

/// The routes behind the palette's modes, with their own state.
pub(crate) fn routes<S: Clone + Send + Sync + 'static>(admin: &str, ctx: RouteCtx) -> Router<S> {
    Router::new()
        .route(&format!("{admin}/palette/modes"), get(modes))
        .route(&format!("{admin}/palette/do"), post(find))
        .route(&format!("{admin}/palette/do/{{code}}"), get(open))
        .route(
            &format!("{admin}/palette/do/{{code}}/preview"),
            post(preview),
        )
        .route(&format!("{admin}/palette/do/{{code}}/run"), post(run))
        .with_state(Arc::new(DoState {
            ctx,
            admin: admin.to_string(),
        }))
}

#[derive(serde::Serialize)]
struct ModeView {
    code: String,
    label: String,
    placeholder: String,
    prefix: Option<char>,
    url: String,
    live: bool,
}

/// The modes this person has beside the finder, in registration order, Do
/// first.
async fn modes(
    State(state): State<Arc<DoState>>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Response {
    let mut found = Vec::new();
    if !by_hand_for(&state, &user).is_empty() {
        found.push(ModeView {
            code: DO.to_string(),
            label: shell.tt(&t!("Do")),
            placeholder: shell.tt(&t!("What do you want to do?")),
            prefix: Some('>'),
            url: format!("{}/palette/do", state.admin),
            live: true,
        });
    }
    for mode in state.ctx.plugin_defined().items::<PaletteMode>() {
        if !mode
            .permission
            .as_deref()
            .is_none_or(|p| user.permissions.allows(p))
        {
            continue;
        }
        found.push(ModeView {
            code: mode.code.clone(),
            label: shell.tt(&mode.label),
            placeholder: mode
                .placeholder
                .as_ref()
                .map(|p| shell.tt(p))
                .unwrap_or_default(),
            prefix: mode.prefix,
            url: format!("{}{}", state.admin, mode.path),
            live: mode.live,
        });
    }
    (
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        axum::Json(found),
    )
        .into_response()
}

/// One input a person types: a flat property of an ability's schema.
struct InputSpec {
    name: String,
    label: String,
    help: String,
    /// `text`, `number`, `check` or `select`.
    kind: &'static str,
    /// Whole numbers only.
    whole: bool,
    /// Each choice as its value and the words shown for it.
    options: Vec<(String, String)>,
    required: bool,
}

/// The inputs of an ability a person can fill by hand: every property a
/// string, a number, a boolean or a choice. `None` for a schema that nests,
/// which only a program calls.
fn by_hand(ability: &Ability) -> Option<Vec<InputSpec>> {
    let schema = ability.input_schema_value();
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut specs = Vec::new();
    for (name, property) in schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        // `"type": ["string", "null"]` is an optional string.
        let kind = match property.get("type") {
            Some(Value::String(one)) => one.as_str(),
            Some(Value::Array(many)) => many
                .iter()
                .filter_map(Value::as_str)
                .find(|t| *t != "null")?,
            _ => return None,
        };
        let options: Vec<(String, String)> = property
            .get("enum")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(|v| (v.to_string(), worded(v))))
            .collect();
        let (kind, whole) = match kind {
            "string" if !options.is_empty() => ("select", false),
            "string" => ("text", false),
            "integer" => ("number", true),
            "number" => ("number", false),
            "boolean" => ("check", false),
            _ => return None,
        };
        let label = worded(name);
        specs.push(InputSpec {
            label: property
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(label),
            help: property
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            kind,
            whole,
            options,
            required: required.contains(&name.as_str()),
            name: name.clone(),
        });
    }
    Some(specs)
}

/// A name as words: `found_on_site` reads "Found on site".
fn worded(name: &str) -> String {
    let mut words = name.replace('_', " ");
    if let Some(first) = words.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    words
}

/// The framework's abilities over any record and any screen, which a
/// program calls and a person has pages for.
fn for_programs(code: &str) -> bool {
    code.starts_with("backend.records.") || code.starts_with("backend.screens.")
}

/// The abilities this person can run from the palette.
fn by_hand_for<'a>(state: &'a DoState, user: &AuthenticatedUser) -> Vec<&'a Ability> {
    state
        .ctx
        .abilities()
        .for_user(user)
        .into_iter()
        .filter(|a| !for_programs(a.code()) && by_hand(a).is_some())
        .collect()
}

fn effect_word(shell: &Shell, effect: Effect) -> String {
    shell.tt(&match effect {
        Effect::Read => t!("Looks up"),
        Effect::Create => t!("Adds"),
        Effect::Update => t!("Changes"),
        Effect::Delete => t!("Deletes"),
        Effect::Send => t!("Sends"),
        _ => t!("Runs"),
    })
}

struct HitView {
    label: String,
    hint: String,
    href: String,
}

#[derive(askama::Template)]
#[template(path = "palette/abilities.html")]
struct AbilitiesTemplate {
    shell: Shell,
    hits: Vec<HitView>,
}

#[derive(serde::Deserialize)]
struct Asked {
    #[serde(default)]
    q: String,
}

/// The abilities matching what was typed.
async fn find(
    State(state): State<Arc<DoState>>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Form(asked): Form<Asked>,
) -> Response {
    let q: String = asked.q.trim().chars().take(120).collect();
    let allowed = by_hand_for(&state, &user);
    let hits = state
        .ctx
        .abilities()
        .search(&user, &q, usize::MAX)
        .into_iter()
        .filter(|a| allowed.iter().any(|b| b.code() == a.code()))
        .take(LIMIT)
        .map(|a| HitView {
            label: shell.tt(a.label()),
            hint: effect_word(&shell, a.effect_kind()),
            href: format!("{}/palette/do/{}", state.admin, a.code()),
        })
        .collect();
    render(AbilitiesTemplate { shell, hits })
}

struct FieldView {
    spec: InputSpec,
    value: String,
}

#[derive(askama::Template)]
#[template(path = "palette/ability_form.html")]
struct FormTemplate {
    label: String,
    describe: String,
    action: String,
    fields: Vec<FieldView>,
    faults: Vec<String>,
    go: String,
}

#[derive(askama::Template)]
#[template(path = "palette/ability_card.html")]
struct CardTemplate {
    shell: Shell,
    label: String,
    summary: String,
    records: Vec<Link>,
    count: Option<u64>,
    /// The filled form's values, by field name.
    draft: Vec<(String, String)>,
    action: String,
    back: String,
    input: String,
    danger: bool,
    go: String,
}

#[derive(askama::Template)]
#[template(path = "palette/ability_done.html")]
struct DoneTemplate {
    shell: Shell,
    label: String,
    ok: bool,
    message: String,
    links: Vec<Link>,
    data: String,
    again: String,
}

fn done(shell: Shell, state: &DoState, ability: &Ability, outcome: Outcome) -> Response {
    // A result that names a page takes the person there. Only a path on this
    // site is followed.
    if let Some(go) = outcome.go.as_deref().filter(|go| {
        go.starts_with('/') && !go.starts_with("//") && !go.contains(['\\', '\r', '\n'])
    }) {
        if let Ok(value) = axum::http::HeaderValue::from_str(go) {
            return (
                [(axum::http::HeaderName::from_static("hx-redirect"), value)],
                (),
            )
                .into_response();
        }
    }
    render(DoneTemplate {
        label: shell.tt(ability.label()),
        ok: true,
        message: shell.tt(&outcome.message),
        links: outcome.links,
        data: outcome
            .data
            .map(|d| serde_json::to_string_pretty(&d).unwrap_or_default())
            .unwrap_or_default(),
        again: format!("{}/palette/do/{}", state.admin, ability.code()),
        shell,
    })
}

/// `ran` is false while only the preview was asked, when nothing can have
/// changed.
fn failed(
    shell: Shell,
    state: &DoState,
    ability: &Ability,
    error: AbilityError,
    ran: bool,
) -> Response {
    let message = match error {
        AbilityError::Refused(text) => shell.tt(&text),
        AbilityError::Invalid(faults) => faults.join(" "),
        AbilityError::Unknown(_) => shell.tt(&t!("That is not something you can do here.")),
        _ if ran => shell.tt(&t!("Something went wrong, and it may not have finished.")),
        _ => shell.tt(&t!("Something went wrong. Nothing was changed.")),
    };
    render(DoneTemplate {
        label: shell.tt(ability.label()),
        ok: false,
        message,
        links: Vec::new(),
        data: String::new(),
        again: format!("{}/palette/do/{}", state.admin, ability.code()),
        shell,
    })
}

fn form(
    shell: Shell,
    state: &DoState,
    ability: &Ability,
    specs: Vec<InputSpec>,
    values: &HashMap<String, String>,
    faults: Vec<String>,
) -> Response {
    render(FormTemplate {
        label: shell.tt(ability.label()),
        describe: ability
            .description()
            .map(|d| shell.tt(d))
            .unwrap_or_default(),
        action: format!("{}/palette/do/{}/preview", state.admin, ability.code()),
        fields: specs
            .into_iter()
            .map(|spec| FieldView {
                value: values
                    .get(&format!("in.{}", spec.name))
                    .cloned()
                    .unwrap_or_default(),
                spec,
            })
            .collect(),
        faults,
        go: shell.tt(&t!("Continue")),
    })
}

fn unknown(shell: &Shell) -> Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        axum::response::Html(format!(
            "<p class=\"lat-palette__empty\">{}</p>",
            askama::filters::escape(
                shell.tt(&t!("That is not something you can do here.")),
                askama::filters::Html
            )
            .map(|s| s.to_string())
            .unwrap_or_default()
        )),
    )
        .into_response()
}

/// The ability's input form, or its card when it takes none.
async fn open(
    State(state): State<Arc<DoState>>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
) -> Response {
    let Some((ability, specs)) = resolve(&state, &user, &code) else {
        return unknown(&shell);
    };
    if specs.is_empty() {
        return shown(&state, shell, user, ability, Value::Object(Map::new())).await;
    }
    form(shell, &state, ability, specs, &HashMap::new(), Vec::new())
}

fn resolve<'a>(
    state: &'a DoState,
    user: &AuthenticatedUser,
    code: &str,
) -> Option<(&'a Ability, Vec<InputSpec>)> {
    let ability = state.ctx.abilities().get(user, code)?;
    if for_programs(code) {
        return None;
    }
    by_hand(ability).map(|specs| (ability, specs))
}

/// The typed values as the ability's input, or the faults in them.
fn gathered(specs: &[InputSpec], values: &HashMap<String, String>) -> Result<Value, Vec<String>> {
    let mut input = Map::new();
    let mut faults = Vec::new();
    for spec in specs {
        let raw = values
            .get(&format!("in.{}", spec.name))
            .map(|v| v.trim())
            .unwrap_or_default();
        if spec.kind == "check" {
            input.insert(spec.name.clone(), Value::Bool(!raw.is_empty()));
            continue;
        }
        if raw.is_empty() {
            continue;
        }
        let value = match (spec.kind, spec.whole) {
            ("number", true) => raw.parse::<i64>().ok().map(Value::from),
            ("number", false) => raw
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            _ => Some(Value::String(raw.to_string())),
        };
        match value {
            Some(value) => {
                input.insert(spec.name.clone(), value);
            }
            None => faults.push(format!("{}: a number is expected", spec.label)),
        }
    }
    if faults.is_empty() {
        Ok(Value::Object(input))
    } else {
        Err(faults)
    }
}

/// Takes the typed input and shows what the ability would do.
async fn preview(
    State(state): State<Arc<DoState>>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(values): Form<HashMap<String, String>>,
) -> Response {
    let Some((ability, specs)) = resolve(&state, &user, &code) else {
        return unknown(&shell);
    };
    let input = match gathered(&specs, &values) {
        Ok(input) => input,
        Err(faults) => return form(shell, &state, ability, specs, &values, faults),
    };
    // The schema's own answer goes back on the form, where it can be fixed.
    if let Err(AbilityError::Invalid(faults)) = state
        .ctx
        .abilities()
        .preview(&user, &code, &input, None)
        .await
    {
        return form(shell, &state, ability, specs, &values, faults);
    }
    shown(&state, shell, user, ability, input).await
}

/// A read runs at once; anything else shows its card and waits.
async fn shown(
    state: &DoState,
    shell: Shell,
    user: AuthenticatedUser,
    ability: &Ability,
    input: Value,
) -> Response {
    let abilities = state.ctx.abilities();
    if !ability.effect_kind().changes() {
        return match abilities.run(&user, ability.code(), input, None).await {
            Ok(outcome) => done(shell, state, ability, outcome),
            Err(e) => failed(shell, state, ability, e, false),
        };
    }
    let preview: Preview = match abilities.preview(&user, ability.code(), &input, None).await {
        Ok(preview) => preview,
        Err(e) => return failed(shell, state, ability, e, false),
    };
    let label = shell.tt(ability.label());
    let mut draft: Vec<(String, String)> = preview
        .draft
        .map(|d| {
            d.values
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(name, v)| (worded(&name), v))
                .collect()
        })
        .unwrap_or_default();
    draft.sort();
    render(CardTemplate {
        summary: preview
            .summary
            .map(|s| shell.tt(&s))
            .or_else(|| ability.description().map(|d| shell.tt(d)))
            .unwrap_or_default(),
        records: preview.records,
        count: preview.count,
        draft,
        action: format!("{}/palette/do/{}/run", state.admin, ability.code()),
        back: format!("{}/palette/do/{}", state.admin, ability.code()),
        input: input.to_string(),
        danger: matches!(ability.effect_kind(), Effect::Delete | Effect::Send),
        go: label.clone(),
        label,
        shell,
    })
}

#[derive(serde::Deserialize)]
struct Confirmed {
    input: String,
}

/// Runs the ability with the input its card carried.
async fn run(
    State(state): State<Arc<DoState>>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(code): Path<String>,
    Form(confirmed): Form<Confirmed>,
) -> Response {
    let Some((ability, _)) = resolve(&state, &user, &code) else {
        return unknown(&shell);
    };
    let input = serde_json::from_str::<Value>(&confirmed.input)
        .unwrap_or_else(|_| Value::Object(Map::new()));
    match state.ctx.abilities().run(&user, &code, input, None).await {
        Ok(outcome) => done(shell, &state, ability, outcome),
        Err(e) => failed(shell, &state, ability, e, true),
    }
}
