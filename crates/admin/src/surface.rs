//! Surfaces: what a screen says it has, in a form a caller can read and
//! drive without knowing what the screen is.
//!
//! A screen is made of components, and each one describes itself the same
//! way: its **controls** (what can be set: a search, a filter, a period), its
//! **parts** (the components inside it: a board's widgets, a widget's own
//! pickers) and its **acts** (what it does once set: show itself, hand back
//! what it holds). A caller asks a screen for that description, then names
//! controls by where they sit and gives values. The component checks them.
//!
//! ```ignore
//! registry.add_surface(Arc::new(Orders));
//! ```
//!
//! Every list a module declares is a surface with no work by its author. A
//! component of your own joins by implementing [`Surface`].
//!
//! Three abilities reach every surface: `backend.screens.list`,
//! `backend.screens.describe` and `backend.screens.act`. An act through
//! them changes nothing; a change is an [`Ability`] of its own.

use std::sync::{Arc, LazyLock};

use laterite_auth::AuthenticatedUser;
use laterite_core::serde_json::{json, Map, Value};
use laterite_core::strata::async_trait;
use laterite_core::{t, Text};

use crate::ability::{Ability, AbilityCx, AbilityError, AbilityHandler, Effect, Outcome};
use crate::routes::RouteCtx;

/// The act that takes the person to the screen with its controls set.
pub const SHOW: &str = "show";
/// The act that answers what the component holds, as data.
pub const READ: &str = "read";

/// One thing a component lets a caller set.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct Control {
    /// What a caller calls it, unique within its part.
    pub name: String,
    /// What a person calls it.
    pub label: Text,
    /// What it takes: `text`, `number`, `boolean`, `date`, `date_range`,
    /// `select`, or a word of the component's own.
    pub kind: String,
    /// What it narrows or changes, when the label does not say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub describe: Option<Text>,
    /// The values it takes, when they are a fixed set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// What it is set to now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

impl Control {
    /// A control called `name`, shown as `label`, taking a `kind` of value.
    pub fn new(name: impl Into<String>, label: impl Into<Text>, kind: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            kind: kind.into(),
            describe: None,
            values: Vec::new(),
            value: None,
        }
    }

    /// Says what the control narrows or changes.
    pub fn describe(mut self, text: impl Into<Text>) -> Self {
        self.describe = Some(text.into());
        self
    }

    /// The fixed set of values it takes.
    pub fn values<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.values = values.into_iter().map(Into::into).collect();
        self
    }

    /// What it is set to now.
    pub fn value(mut self, value: impl Into<Value>) -> Self {
        self.value = Some(value.into());
        self
    }
}

/// A component as a caller sees it: its controls, the components inside it
/// and what it does.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct Part {
    /// What a caller calls it, unique among its siblings.
    pub name: String,
    /// What a person calls it.
    pub label: Text,
    /// What it shows, for a caller choosing among parts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub describe: Option<Text>,
    /// What can be set on it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<Control>,
    /// The components inside it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
    /// What it does once set: [`SHOW`], [`READ`], or a word of its own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub acts: Vec<String>,
}

impl Part {
    /// A part called `name`, shown as `label`.
    pub fn new(name: impl Into<String>, label: impl Into<Text>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            describe: None,
            controls: Vec::new(),
            parts: Vec::new(),
            acts: Vec::new(),
        }
    }

    /// Says what the part shows.
    pub fn describe(mut self, text: impl Into<Text>) -> Self {
        self.describe = Some(text.into());
        self
    }

    /// Adds something a caller can set.
    pub fn control(mut self, control: Control) -> Self {
        self.controls.push(control);
        self
    }

    /// Adds a component inside this one.
    pub fn part(mut self, part: Part) -> Self {
        self.parts.push(part);
        self
    }

    /// Adds something the part does: [`SHOW`], [`READ`], or its own.
    pub fn act(mut self, act: impl Into<String>) -> Self {
        self.acts.push(act.into());
        self
    }

    /// The part at `path` under this one; this one for an empty path.
    pub fn find(&self, path: &[String]) -> Option<&Part> {
        match path.split_first() {
            None => Some(self),
            Some((name, rest)) => self.parts.iter().find(|p| p.name == *name)?.find(rest),
        }
    }
}

/// What a caller asks a surface to do.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Request {
    /// The part to act on, from the surface down; empty for the surface
    /// itself.
    pub part: Vec<String>,
    /// The act, one the part lists.
    pub act: String,
    /// The controls to set first, by name.
    pub set: Map<String, Value>,
}

impl Request {
    /// A request for `act` on the surface itself, with nothing set.
    pub fn new(act: impl Into<String>) -> Self {
        Self {
            part: Vec::new(),
            act: act.into(),
            set: Map::new(),
        }
    }

    /// Names the part to act on, from the surface down.
    pub fn part<I, S>(mut self, path: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.part = path.into_iter().map(Into::into).collect();
        self
    }

    /// Sets a control before the act.
    pub fn set(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.set.insert(name.into(), value.into());
        self
    }

    /// The value asked for the control `name`, as text: a string as it is,
    /// a number or a boolean written out.
    pub fn text(&self, name: &str) -> Option<String> {
        match self.set.get(name)? {
            Value::String(text) => Some(text.trim().to_string()),
            Value::Bool(yes) => Some(if *yes { "1" } else { "0" }.to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }
}

/// A screen that describes itself to a caller and acts on what it is told.
#[async_trait]
pub trait Surface: Send + Sync {
    /// Where the screen is, under the admin root (`/acme/orders`). It names
    /// the surface.
    fn path(&self) -> &str;

    /// What a person calls the screen.
    fn label(&self) -> Text;

    /// What the screen is for, for a caller choosing among screens.
    fn describe(&self) -> Option<Text> {
        None
    }

    /// The permission a person needs to see the screen.
    fn permission(&self) -> Option<&str> {
        None
    }

    /// The screen as this person sees it: its controls with what they are
    /// set to, its parts and its acts. Leave out what they may not use.
    async fn parts(&self, cx: &AbilityCx<'_>) -> Result<Part, AbilityError>;

    /// Sets the controls a request names and does its act, changing nothing
    /// stored. A control or a value the part does not take is refused by
    /// name with [`AbilityError::invalid`], so the caller can ask again.
    async fn act(&self, cx: &AbilityCx<'_>, request: &Request) -> Result<Outcome, AbilityError>;
}

/// A registered surface.
#[derive(Clone)]
pub struct SurfaceReg {
    /// The surface.
    pub surface: Arc<dyn Surface>,
}

impl SurfaceReg {
    /// Registers `surface`.
    pub fn new(surface: Arc<dyn Surface>) -> Self {
        Self { surface }
    }
}

impl std::fmt::Debug for SurfaceReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceReg")
            .field("path", &self.surface.path())
            .finish()
    }
}

/// One surface open to a person: a module's own, or a list.
enum Found<'a> {
    Own(&'a dyn Surface),
    List(&'a crate::records::RecordKind),
}

impl Found<'_> {
    fn summary(&self) -> Value {
        match self {
            Self::Own(surface) => json!({
                "screen": surface.path(),
                "label": surface.label().source(),
                "describe": surface.describe().as_ref().map(Text::source),
            }),
            Self::List(kind) => kind.surface_summary(),
        }
    }
}

fn open_to<'a>(ctx: &'a RouteCtx, user: &AuthenticatedUser) -> Vec<Found<'a>> {
    let own = ctx
        .plugin_defined()
        .items::<SurfaceReg>()
        .into_iter()
        .map(|reg| reg.surface.as_ref())
        .filter(|s| s.permission().is_none_or(|p| user.permissions.allows(p)))
        .map(Found::Own);
    let lists = ctx
        .records()
        .iter()
        .filter(|k| k.sees(user))
        .map(Found::List);
    own.chain(lists).collect()
}

/// Whether any surface is open to `user`: without one the screen abilities
/// are not offered.
pub(crate) fn any_for(ctx: &RouteCtx, user: &AuthenticatedUser) -> bool {
    !open_to(ctx, user).is_empty()
}

/// The surface `screen` names, when the person may see it. One answer for a
/// screen that does not exist and one they may not see.
fn found<'a>(cx: &AbilityCx<'a>, screen: &str) -> Result<Found<'a>, AbilityError> {
    open_to(cx.ctx(), cx.user())
        .into_iter()
        .find(|found| match found {
            Found::Own(surface) => surface.path() == screen,
            Found::List(kind) => kind.key == screen,
        })
        .ok_or_else(|| {
            AbilityError::invalid(format!(
                "no screen is named `{screen}`: list them with backend.screens.list"
            ))
        })
}

struct List;

#[async_trait]
impl AbilityHandler for List {
    async fn run(&self, cx: &AbilityCx<'_>, _input: Value) -> Result<Outcome, AbilityError> {
        let screens: Vec<Value> = open_to(cx.ctx(), cx.user())
            .iter()
            .map(Found::summary)
            .collect();
        Ok(Outcome::new(t!("The screens.")).data(json!(screens)))
    }
}

struct Describe;

#[async_trait]
impl AbilityHandler for Describe {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let screen = input["screen"].as_str().unwrap_or_default();
        let part = match found(cx, screen)? {
            Found::Own(surface) => surface.parts(cx).await?,
            Found::List(kind) => kind.surface_part(cx.user()),
        };
        let mut data = serde_json::to_value(&part)
            .map_err(|e| AbilityError::Failed(format!("surface `{screen}`: {e}")))?;
        data["screen"] = json!(screen);
        Ok(Outcome::new(t!("What the screen has.")).data(data))
    }
}

struct Act;

#[async_trait]
impl AbilityHandler for Act {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let screen = input["screen"].as_str().unwrap_or_default();
        let request = Request {
            part: input["part"]
                .as_str()
                .unwrap_or_default()
                .split('/')
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect(),
            act: input["act"].as_str().unwrap_or(SHOW).to_string(),
            set: input["set"].as_object().cloned().unwrap_or_default(),
        };
        match found(cx, screen)? {
            Found::Own(surface) => {
                // The part and the act are checked against what the surface
                // says it has, so one that names neither never reaches it.
                let whole = surface.parts(cx).await?;
                let part = whole.find(&request.part).ok_or_else(|| {
                    AbilityError::invalid(format!(
                        "part: `{}` is not a part of this screen",
                        request.part.join("/")
                    ))
                })?;
                if !part.acts.contains(&request.act) {
                    return Err(AbilityError::invalid(format!(
                        "act: this part does not `{}`; it offers: {}",
                        request.act,
                        part.acts.join(", ")
                    )));
                }
                let unknown: Vec<String> = request
                    .set
                    .keys()
                    .filter(|name| !part.controls.iter().any(|c| c.name == **name))
                    .map(|name| format!("set/{name}: this part has no such control"))
                    .collect();
                if !unknown.is_empty() {
                    return Err(AbilityError::Invalid(unknown));
                }
                surface.act(cx, &request).await
            }
            Found::List(kind) => kind.surface_act(cx, &request).await,
        }
    }
}

fn screen_property() -> Value {
    json!({
        "type": "string",
        "description": "The screen, as backend.screens.list names it (`/acme/orders`)."
    })
}

/// The abilities over surfaces, offered to a person with at least one
/// surface open to them.
pub(crate) static ABILITIES: LazyLock<Vec<Ability>> = LazyLock::new(|| {
    vec![
        Ability::new("backend.screens.list", t!("List the screens"), Arc::new(List))
            .describe(t!(
                "Lists the screens this person can look at: lists, dashboards and others, each by its path."
            ))
            .input_schema(json!({ "type": "object", "additionalProperties": false }))
            .effect(Effect::Read),
        Ability::new(
            "backend.screens.describe",
            t!("Describe a screen"),
            Arc::new(Describe),
        )
        .describe(t!(
            "Answers what one screen has: its controls with what they are set to, the parts inside it, and what each part does."
        ))
        .input_schema(json!({
            "type": "object",
            "properties": { "screen": screen_property() },
            "required": ["screen"],
            "additionalProperties": false
        }))
        .effect(Effect::Read),
        Ability::new("backend.screens.act", t!("Use a screen"), Arc::new(Act))
            .describe(t!(
                "Sets controls on a screen or one of its parts, then shows it to the person or reads what it holds. Describe the screen first. Nothing stored is changed."
            ))
            .input_schema(json!({
                "type": "object",
                "properties": {
                    "screen": screen_property(),
                    "part": {
                        "type": "string",
                        "description": "The part to act on, by the names from the screen down, joined with `/`. Leave out for the screen itself."
                    },
                    "set": {
                        "type": "object",
                        "description": "The controls to set, by name, each with its value."
                    },
                    "act": {
                        "type": "string",
                        "description": "`show` takes the person there; `read` answers the data; or another act the part lists."
                    }
                },
                "required": ["screen", "act"],
                "additionalProperties": false
            }))
            .effect(Effect::Read),
    ]
});
