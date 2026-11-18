# Abilities

An ability is one act a module declares by name, with a typed input and a
permission, for any caller to run as the signed-in person.

## Declare an ability

```rust
use laterite_admin::ability::{Ability, AbilityCx, AbilityError, AbilityHandler, Effect, Link, Outcome};
use laterite_admin::AdminRegistry;
use laterite_core::serde_json::Value;
use laterite_core::t;
use std::sync::Arc;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddLead {
    /// The person's full name.
    name: String,
    /// A phone number with its country code.
    phone: Option<String>,
}

struct AddLeadHandler;

#[laterite_core::strata::async_trait]
impl AbilityHandler for AddLeadHandler {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let lead: AddLead = laterite_core::serde_json::from_value(input)
            .map_err(|e| AbilityError::invalid(e.to_string()))?;
        let id = acme::leads::add(cx.db(), &lead.name, lead.phone.as_deref(), cx.actor())
            .await
            .map_err(|e| AbilityError::Failed(e.to_string()))?;
        Ok(Outcome::new(t!("Added the lead."))
            .link(Link::new(lead.name, format!("/admin/acme/crm/leads/{id}/edit"))))
    }
}

fn register(&self, registry: &mut laterite_core::Registry) {
    registry.add_ability(
        Ability::new("acme.crm.add_lead", t!("Add a lead"), Arc::new(AddLeadHandler))
            .describe(t!("Adds a person to the leads list."))
            .permission("acme.crm.leads.create")
            .input::<AddLead>()
            .sensitive(["phone"])
            .effect(Effect::Create),
    );
}
```

Method | Sets | Default
--- | --- | ---
`new(code, label, handler)` | the dotted code with the module's id first, the name people see, the code behind it | required
`describe` | what it does and when to use it, for a caller choosing among abilities | none
`group` | the shelf it sits on in a catalogue | the code without its last segment
`permission` | the permission a person needs to see and run it | open to every operator
`input::<T>()` | the input as a type deriving `schemars::JsonSchema` | no input
`input_schema(value)` | the input as a JSON Schema object | no input
`effect` | what running it does | `Effect::Run`
`untrusted()` | marks the result as carrying text written by people outside the panel | trusted
`sensitive([..])` | input fields left out of the audit entry | none

Effect | Meaning
--- | ---
`Read` | answers a question and changes nothing
`Create` | adds a record
`Update` | changes a record
`Delete` | removes a record
`Send` | sends something to a person outside the panel
`Run` | any other change: publishing, importing, a rebuild

An ability with an effect other than `Read` names a permission. A doc comment
on an input field is that field's description in the schema.

## Say what it would do

```rust
use laterite_admin::ability::{Draft, Preview};

async fn preview(&self, cx: &AbilityCx<'_>, input: &Value) -> Result<Preview, AbilityError> {
    let version = acme::documents::draft_version(cx.db(), input).await?;
    Ok(Preview::new(t!("Publishes the draft of Terms of Use."))
        .record(Link::new(version.title, version.href))
        .count(1))
}
```

Method | Sets
--- | ---
`Preview::new(summary)` | the act in a sentence
`record(link)` | a record the act touches
`count(n)` | how many records it touches
`draft(Draft::new(path, id, values))` | a resource's form, filled in, for the person to save

## Answer from the handler

Return | When
--- | ---
`Ok(Outcome::new(message))` | it ran; add `.link(..)` for records and `.data(..)` for a result as JSON
`Err(AbilityError::Refused(text))` | it declined, with a reason the person reads
`Err(AbilityError::invalid(message))` | the input is wrong in a way the schema cannot state
`Err(AbilityError::Failed(message))` | it broke; the message goes to the log

`AbilityCx` gives `db()`, `ctx()`, `user()` and `actor()`. Pass `actor()` to
`FormHandle::create` and `FormHandle::update`, and to `AuditEntry::new`.

## Run an ability

```rust
use laterite_core::serde_json::json;

let abilities = ctx.abilities();

for ability in abilities.for_user(&user) {
    println!("{} ({})", ability.code(), ability.effect_kind().as_str());
}
let found = abilities.search(&user, "add a lead", 5);

let preview = abilities
    .preview(&user, "acme.crm.add_lead", &json!({ "name": "Asha Rao" }), None)
    .await?;
let outcome = abilities
    .run(&user, "acme.crm.add_lead", json!({ "name": "Asha Rao" }), Some("acme.assistant"))
    .await?;
```

Method | Answers
--- | ---
`for_user(&user)` | every ability the person may run
`get(&user, code)` | one ability, when the person may run it
`search(&user, words, limit)` | the abilities matching the words, best first
`preview(&user, code, &input, channel)` | the handler's `Preview`
`run(&user, code, input, channel)` | the handler's `Outcome`

`channel` names what acts for the person (`acme.assistant`), or `None`. The
Audit Log shows it under Through.

Error | Cause
--- | ---
`AbilityError::Unknown(code)` | no ability has the code, or the person lacks its permission
`AbilityError::Invalid(faults)` | the input does not fit the schema; one message per fault

A run with an effect other than `Read` writes an `ability.run` audit entry.

## Offer a resource's records

```yaml
entity: posts
path: /posts
title: Posts
permission: acme.posts.manage
abilities:
  describe: Blog posts. A post has a title, a body in markdown and a publish date.
  update: false
  hide: [internal_notes]
```

```rust
use laterite_admin::records::RecordAbilities;

Resource::new("/posts", "Posts", list)
    .abilities(RecordAbilities::new().describe(t!("Blog posts.")).hide(["internal_notes"]));
```

Key | Sets | Default
--- | --- | ---
`abilities: false` | keeps the resource out of every record ability | offered
`describe` | what the records are, for a caller choosing where to look | none
`create` | a new record may be drafted | `true`, where the list is `creatable`
`update` | a change may be drafted | `true`
`hide` | fields and columns kept from a caller | none
`untrusted` | the records carry text written by people outside the panel | `false`

Ability | Input | Answers
--- | --- | ---
`backend.records.kinds` | `resource` (optional) | the kinds open to the person; one kind with its fields and columns
`backend.records.find` | `resource`, `search`, `limit` | matching rows with their ids, 25 at most
`backend.records.read` | `resource`, `id` | one record's field values
`backend.records.create` | `resource`, `values` | `preview`: the filled form as a `Draft`; `run`: the new id
`backend.records.update` | `resource`, `id`, `values` | `preview`: the changed form as a `Draft`; `run`: the id

`resource` is the resource's `path`. Each ability asks for the resource's
`permission`, and a create or an update for its `permissions.create` or
`permissions.update`. `password` and `sensitive` fields are never read or set.

## Act for a person in your own writes

```rust
use laterite_core::Actor;

let actor = Actor::from(&user).through("acme.assistant");
form.create(values, &actor).await?;
```

`Actor::channel()` reads it back in a model listener.
