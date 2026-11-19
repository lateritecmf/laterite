# Surfaces

A surface is a screen describing itself to a caller: its controls, the parts
inside it and what each part does.

## Describe a component

```rust
use laterite_admin::ability::{AbilityCx, AbilityError, Outcome};
use laterite_admin::surface::{Control, Part, Request, Surface, READ, SHOW};
use laterite_admin::AdminRegistry;
use laterite_core::serde_json::json;
use laterite_core::{t, Text};
use std::sync::Arc;

struct Overview;

#[laterite_core::strata::async_trait]
impl Surface for Overview {
    fn path(&self) -> &str {
        "/acme/overview"
    }

    fn label(&self) -> Text {
        t!("Overview")
    }

    fn permission(&self) -> Option<&str> {
        Some("acme.reports.view")
    }

    async fn parts(&self, _cx: &AbilityCx<'_>) -> Result<Part, AbilityError> {
        Ok(Part::new("overview", t!("Overview")).act(SHOW).part(
            Part::new("orders_by_day", t!("Orders by day"))
                .describe(t!("How many orders arrived each day."))
                .control(
                    Control::new("range", t!("Period"), "date_range")
                        .value(json!({ "from": "2026-10-01", "to": "2026-10-31" })),
                )
                .control(Control::new("group_by", t!("Group by"), "select").values(["day", "week"]))
                .act(READ),
        ))
    }

    async fn act(&self, cx: &AbilityCx<'_>, request: &Request) -> Result<Outcome, AbilityError> {
        if request.act == SHOW {
            return Ok(Outcome::new(t!("Opened the overview.")).go("/admin/acme/overview"));
        }
        let from = request.set["range"]["from"]
            .as_str()
            .ok_or_else(|| AbilityError::invalid("set/range: give `from` and `to`"))?;
        let points = acme::orders::by_day(cx.db(), from, request.text("group_by")).await?;
        Ok(Outcome::new(t!("The orders by day.")).data(json!({ "points": points })))
    }
}

fn register(&self, registry: &mut laterite_core::Registry) {
    registry.add_surface(Arc::new(Overview));
}
```

Method | Answers | Default
--- | --- | ---
`path` | where the screen is under the admin root; it names the surface | required
`label` | what a person calls the screen | required
`describe` | what the screen is for | none
`permission` | the permission a person needs to see it | every signed-in person
`parts` | the screen as this person sees it | required
`act` | sets the controls a request names and does its act | required

## Build the description

`Part` | Sets
--- | ---
`Part::new(name, label)` | the name a caller uses, the name a person reads
`describe(text)` | what the part shows
`control(control)` | something a caller sets
`part(part)` | a component inside this one
`act(name)` | something the part does: `SHOW`, `READ`, or a word of its own

`Control` | Sets
--- | ---
`Control::new(name, label, kind)` | the name, the label, and `text`, `number`, `boolean`, `date`, `date_range`, `select` or a word of its own
`describe(text)` | what it narrows or changes
`values([..])` | the fixed set of values it takes
`value(value)` | what it is set to now

Act | Does
--- | ---
`SHOW` (`show`) | answers `Outcome::go` with the screen's address, the controls set
`READ` (`read`) | answers `Outcome::data` with what the part holds

## Read a request

```rust
let part: &[String] = &request.part;      // ["orders_by_day"]
let act: &str = &request.act;             // "read"
let grouping = request.text("group_by");  // Some("day")
let range = &request.set["range"];        // { "from": .., "to": .. }
```

Field | Holds
--- | ---
`part` | the names from the surface down to the part; empty for the surface itself
`act` | an act that part lists
`set` | the controls to set, by name; each one the part lists

Return `AbilityError::invalid("set/range: ..")` for a value the control does
not take.

## Call a surface

```rust
use laterite_core::serde_json::json;

let abilities = ctx.abilities();
let screens = abilities.run(&user, "backend.screens.list", json!({}), None).await?;
let described = abilities
    .run(&user, "backend.screens.describe", json!({ "screen": "/acme/overview" }), None)
    .await?;
let read = abilities
    .run(&user, "backend.screens.act", json!({
        "screen": "/acme/overview",
        "part": "orders_by_day",
        "act": "read",
        "set": { "range": { "from": "2026-11-01", "to": "2026-11-15" }, "group_by": "day" }
    }), None)
    .await?;
```

Ability | Input | Answers
--- | --- | ---
`backend.screens.list` | none | every screen open to the person: `screen`, `label`, `describe`
`backend.screens.describe` | `screen` | its controls, parts and acts
`backend.screens.act` | `screen`, `part` (names joined with `/`), `set`, `act` | the act's `Outcome`

```json
{
  "screen": "/acme/overview", "name": "overview", "label": "Overview", "acts": ["show"],
  "parts": [{
    "name": "orders_by_day", "label": "Orders by day", "acts": ["read"],
    "controls": [
      { "name": "range", "label": "Period", "kind": "date_range",
        "value": { "from": "2026-10-01", "to": "2026-10-31" } },
      { "name": "group_by", "label": "Group by", "kind": "select", "values": ["day", "week"] }
    ]
  }]
}
```

## Lists

Every resource that offers [record abilities](abilities.md) is a surface
named by its `path`.

Control | Is
--- | ---
`search` | the list's search
each [filter](../lists/filters.md) by its key | that filter, with its `values` for a `select`
`sort`, `dir` | a sortable column, and `asc` or `desc`

```json
{ "screen": "/posts", "act": "show", "set": { "status": "live", "views_min": 1000, "sort": "views", "dir": "desc" } }
```
