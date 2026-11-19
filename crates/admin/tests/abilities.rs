//! Abilities declared by a module, listed for a person and run as them.
//! Imports only the public API.

use std::sync::Arc;

use laterite_admin::ability::{
    Ability, AbilityCx, AbilityError, AbilityHandler, Confirm, Effect, Outcome, OwnPolicy,
    PanelPolicy, PolicyRule,
};
use laterite_admin::routes::RouteCtx;
use laterite_admin::settings::store;
use laterite_admin::AdminRegistry;
use laterite_auth::{
    AuthConfig, AuthService, AuthenticatedUser, BackendUser, NewOperator, PermissionSet,
};
use laterite_core::serde_json::{json, Value};
use laterite_core::strata::async_trait;
use laterite_core::{t, Db, ModuleId, Registry};

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddLead {
    /// The person's name.
    name: String,
    /// A phone number with its country code.
    phone: Option<String>,
}

/// Answers with the name it was given and who it ran as.
struct AddLeadHandler;

#[async_trait]
impl AbilityHandler for AddLeadHandler {
    async fn run(&self, cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        let lead: AddLead =
            serde_json::from_value(input).map_err(|e| AbilityError::invalid(e.to_string()))?;
        if lead.name == "Nobody" {
            return Err(AbilityError::Refused(t!("That person is already a lead.")));
        }
        Ok(Outcome::new(t!("Added the lead.")).data(json!({
            "name": lead.name,
            "phone": lead.phone,
            "as": cx.user().user.username,
            "through": cx.actor().channel(),
        })))
    }
}

struct CountHandler;

#[async_trait]
impl AbilityHandler for CountHandler {
    async fn run(&self, _cx: &AbilityCx<'_>, _input: Value) -> Result<Outcome, AbilityError> {
        Ok(Outcome::new(t!("There are 3 leads.")))
    }
}

fn registry() -> Arc<Registry> {
    let mut registry = Registry::new();
    registry.set_owner(ModuleId::new("acme.crm"));
    registry.add_ability(
        Ability::new(
            "acme.crm.add_lead",
            t!("Add a lead"),
            Arc::new(AddLeadHandler),
        )
        .describe(t!("Adds a person to the leads list."))
        .permission("acme.crm.leads.create")
        .input::<AddLead>()
        .sensitive(["phone"])
        .effect(Effect::Create),
    );
    registry.add_ability(
        Ability::new(
            "acme.crm.count_leads",
            t!("Count the leads"),
            Arc::new(CountHandler),
        )
        .describe(t!("Says how many people are on the leads list."))
        .effect(Effect::Read),
    );
    registry.add_ability(
        Ability::new(
            "acme.crm.remove_lead",
            t!("Remove a lead"),
            Arc::new(CountHandler),
        )
        .permission("acme.crm.leads.delete")
        .effect(Effect::Delete),
    );
    Arc::new(registry)
}

async fn boot() -> (
    RouteCtx,
    AuthService,
    i64,
    laterite_core::testing::TestGuard,
) {
    let (db, guard): (Db, _) =
        laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await;
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    let id = auth
        .create_superuser(NewOperator {
            username: "asha",
            email: "asha@acme.test",
            first_name: "Asha",
            last_name: None,
            password: "correct horse 42",
            timezone: None,
        })
        .await
        .unwrap();
    let ctx = RouteCtx::builder(db)
        .auth(auth.clone())
        .contributions(registry())
        .build();
    (ctx, auth, id, guard)
}

fn person(id: i64, grants: &[&str]) -> AuthenticatedUser {
    let now = chrono::Utc::now();
    AuthenticatedUser {
        user: BackendUser {
            id,
            username: "asha".to_string(),
            email: "asha@acme.test".to_string(),
            first_name: "Asha".to_string(),
            last_name: None,
            password_hash: String::new(),
            is_superuser: false,
            is_active: true,
            timezone: None,
            locale: None,
            created_at: now,
            updated_at: now,
        },
        permissions: PermissionSet::new(false, grants.iter().map(|g| g.to_string())),
    }
}

#[tokio::test]
async fn a_person_sees_only_what_their_permissions_allow() {
    let (ctx, _auth, id, _guard) = boot().await;
    let abilities = ctx.abilities();

    let reader = person(id, &[]);
    let codes: Vec<&str> = abilities
        .for_user(&reader)
        .iter()
        .map(|a| a.code())
        .collect();
    assert_eq!(codes, ["acme.crm.count_leads"]);
    assert!(abilities.get(&reader, "acme.crm.add_lead").is_none());

    let writer = person(id, &["acme.crm.leads.create"]);
    assert_eq!(abilities.for_user(&writer).len(), 2);
    let found = abilities.search(&writer, "add someone as a lead", 5);
    assert_eq!(found[0].code(), "acme.crm.add_lead");
    assert_eq!(found[0].group_code(), "acme.crm");
}

#[tokio::test]
async fn running_without_the_permission_answers_as_if_it_did_not_exist() {
    let (ctx, auth, id, _guard) = boot().await;
    let reader = person(id, &[]);

    let refused = ctx
        .abilities()
        .run(
            &reader,
            "acme.crm.add_lead",
            json!({ "name": "Ravi" }),
            None,
        )
        .await;
    assert!(matches!(refused, Err(AbilityError::Unknown(code)) if code == "acme.crm.add_lead"));
    assert!(auth.recent_audit(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_input_that_does_not_fit_the_schema_runs_nothing() {
    let (ctx, auth, id, _guard) = boot().await;
    let writer = person(id, &["acme.crm.leads.create"]);

    let refused = ctx
        .abilities()
        .run(&writer, "acme.crm.add_lead", json!({ "phone": 5 }), None)
        .await;
    let Err(AbilityError::Invalid(faults)) = refused else {
        panic!("expected a refusal");
    };
    assert!(faults.iter().any(|f| f.contains("name")), "{faults:?}");
    assert!(faults.iter().any(|f| f.starts_with("/phone")), "{faults:?}");
    assert!(auth.recent_audit(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_change_runs_as_the_person_and_is_recorded_with_its_channel() {
    let (ctx, auth, id, _guard) = boot().await;
    let writer = person(id, &["acme.crm.leads.create"]);

    let outcome = ctx
        .abilities()
        .run(
            &writer,
            "acme.crm.add_lead",
            json!({ "name": "Ravi", "phone": "+15550100" }),
            Some("acme.assistant"),
        )
        .await
        .unwrap();
    let data = outcome.data.unwrap();
    assert_eq!(data["as"], "asha");
    assert_eq!(data["through"], "acme.assistant");

    let entries = auth.recent_audit(10).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, "ability.run");
    assert_eq!(entries[0].actor_user_id, Some(id));
    assert_eq!(entries[0].channel.as_deref(), Some("acme.assistant"));
    assert_eq!(entries[0].target_id.as_deref(), Some("acme.crm.add_lead"));
    let detail: Value = serde_json::from_str(entries[0].detail.as_deref().unwrap()).unwrap();
    assert_eq!(detail["effect"], "create");
    assert_eq!(detail["input"]["name"], "Ravi");
    // A sensitive field never reaches the log.
    assert_eq!(detail["input"]["phone"], "[kept out]");
}

#[tokio::test]
async fn a_read_and_a_refusal_leave_no_entry() {
    let (ctx, auth, id, _guard) = boot().await;
    let writer = person(id, &["acme.crm.leads.create"]);
    let abilities = ctx.abilities();

    abilities
        .run(&writer, "acme.crm.count_leads", json!({}), None)
        .await
        .unwrap();
    let refused = abilities
        .run(
            &writer,
            "acme.crm.add_lead",
            json!({ "name": "Nobody" }),
            None,
        )
        .await;
    assert!(matches!(refused, Err(AbilityError::Refused(_))));
    // An input the ability does not declare is refused before it runs.
    let extra = abilities
        .run(
            &writer,
            "acme.crm.count_leads",
            json!({ "drop": "everything" }),
            None,
        )
        .await;
    assert!(matches!(extra, Err(AbilityError::Invalid(_))));
    assert!(auth.recent_audit(10).await.unwrap().is_empty());
}

const ALL: &[&str] = &["acme.crm.leads.create", "acme.crm.leads.delete"];

async fn panel(ctx: &RouteCtx, rules: &[(&str, Confirm)]) {
    let rules = rules
        .iter()
        .map(|(what, confirm)| PolicyRule::new(*what, *confirm))
        .collect();
    store::save(ctx.db(), &PanelPolicy { rules }).await.unwrap();
}

async fn own(ctx: &RouteCtx, id: i64, rules: &[(&str, Confirm)]) {
    let rules = rules
        .iter()
        .map(|(what, confirm)| PolicyRule::new(*what, *confirm))
        .collect();
    store::save_for(ctx.db(), id, &OwnPolicy { rules })
        .await
        .unwrap();
}

async fn offered(ctx: &RouteCtx, user: &AuthenticatedUser) -> Vec<(String, Confirm)> {
    ctx.abilities()
        .offered(user)
        .await
        .unwrap()
        .into_iter()
        .map(|(ability, confirm)| (ability.code().to_string(), confirm))
        .collect()
}

#[tokio::test]
async fn with_no_rule_the_effect_decides_how_far_a_channel_goes() {
    let (ctx, auth, id, _guard) = boot().await;
    let user = person(id, ALL);
    let abilities = ctx.abilities();

    assert_eq!(
        offered(&ctx, &user).await,
        [
            ("acme.crm.add_lead".to_string(), Confirm::Ask),
            ("acme.crm.count_leads".to_string(), Confirm::Auto),
        ]
    );
    // A delete is not offered to a channel, to see or to run.
    let hidden = abilities.confirm(&user, "acme.crm.remove_lead").await;
    assert!(matches!(hidden, Err(AbilityError::Unknown(_))));
    for channel in [Some("acme.assistant"), None] {
        let ran = abilities
            .run(&user, "acme.crm.remove_lead", json!({}), channel)
            .await;
        // The person themself is not bound by the policy.
        assert_eq!(ran.is_ok(), channel.is_none(), "{channel:?}");
    }
    let previewed = abilities
        .preview(
            &user,
            "acme.crm.remove_lead",
            &json!({}),
            Some("acme.assistant"),
        )
        .await;
    assert!(matches!(previewed, Err(AbilityError::Unknown(_))));

    // A run through a channel records how far it was allowed to go.
    abilities
        .run(
            &user,
            "acme.crm.add_lead",
            json!({ "name": "Ravi" }),
            Some("acme.assistant"),
        )
        .await
        .unwrap();
    let entries = auth.recent_audit(10).await.unwrap();
    let through: Vec<Value> = entries
        .iter()
        .filter(|e| e.channel.is_some())
        .map(|e| serde_json::from_str(e.detail.as_deref().unwrap()).unwrap())
        .collect();
    assert_eq!(through.len(), 1);
    assert_eq!(through[0]["confirm"], "ask");
}

#[tokio::test]
async fn the_most_exact_rule_of_the_panel_decides() {
    let (ctx, _auth, id, _guard) = boot().await;
    let user = person(id, ALL);

    panel(
        &ctx,
        &[
            ("effect:create", Confirm::Off),
            ("acme.crm", Confirm::Ask),
            ("acme.crm.add_lead", Confirm::Auto),
            ("acme.crm.remove_lead", Confirm::Ask),
            // The start of a code counts only up to a dot.
            ("acme.crm.count", Confirm::Off),
        ],
    )
    .await;
    assert_eq!(
        offered(&ctx, &user).await,
        [
            ("acme.crm.add_lead".to_string(), Confirm::Auto),
            ("acme.crm.count_leads".to_string(), Confirm::Ask),
            ("acme.crm.remove_lead".to_string(), Confirm::Ask),
        ]
    );

    // A row said twice never loosens.
    panel(
        &ctx,
        &[
            ("acme.crm.add_lead", Confirm::Auto),
            ("acme.crm.add_lead", Confirm::Off),
        ],
    )
    .await;
    let confirm = ctx.abilities().confirm(&user, "acme.crm.add_lead").await;
    assert!(matches!(confirm, Err(AbilityError::Unknown(_))));
}

#[tokio::test]
async fn a_person_tightens_freely_and_loosens_only_to_the_panels_rule() {
    let (ctx, _auth, id, _guard) = boot().await;
    let user = person(id, ALL);
    let abilities = ctx.abilities();

    panel(&ctx, &[("acme.crm.add_lead", Confirm::Auto)]).await;
    own(
        &ctx,
        id,
        &[
            ("acme.crm.add_lead", Confirm::Ask),
            ("acme.crm.count_leads", Confirm::Off),
            // The panel leaves a delete off; a person cannot turn it on.
            ("acme.crm.remove_lead", Confirm::Auto),
        ],
    )
    .await;
    assert_eq!(
        offered(&ctx, &user).await,
        [("acme.crm.add_lead".to_string(), Confirm::Ask)]
    );

    // Once the panel allows a delete with a question, their rule reaches
    // that far and no further.
    panel(&ctx, &[("effect:delete", Confirm::Ask)]).await;
    assert_eq!(
        abilities
            .confirm(&user, "acme.crm.remove_lead")
            .await
            .unwrap(),
        Confirm::Ask
    );

    // Someone else keeps the panel's answer.
    let other = person(id + 1, ALL);
    assert_eq!(
        abilities
            .confirm(&other, "acme.crm.count_leads")
            .await
            .unwrap(),
        Confirm::Auto
    );
}
