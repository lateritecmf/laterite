//! Abilities declared by a module, listed for a person and run as them.
//! Imports only the public API.

use std::sync::Arc;

use laterite_admin::ability::{
    Ability, AbilityCx, AbilityError, AbilityHandler, Confirm, Effect, Outcome, OwnPolicy,
    PanelPolicy, PolicyRule,
};
use laterite_admin::routes::RouteCtx;
use laterite_admin::settings::store;
use laterite_admin::surface::{Control, Part, Request, Surface, READ, SHOW};
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
    registry.add_surface(Arc::new(Overview));
    Arc::new(registry)
}

/// A dashboard of a module's own: one chart with a period and a grouping.
struct Overview;

#[async_trait]
impl Surface for Overview {
    fn path(&self) -> &str {
        "/acme/overview"
    }

    fn label(&self) -> laterite_core::Text {
        t!("Overview")
    }

    fn permission(&self) -> Option<&str> {
        Some("acme.crm.reports")
    }

    async fn parts(&self, _cx: &AbilityCx<'_>) -> Result<Part, AbilityError> {
        Ok(Part::new("overview", t!("Overview")).act(SHOW).part(
            Part::new("leads_by_day", t!("Leads by day"))
                .describe(t!("How many leads arrived each day."))
                .control(
                    Control::new("range", t!("Period"), "date_range")
                        .value(json!({ "from": "2026-10-01", "to": "2026-10-31" })),
                )
                .control(Control::new("group_by", t!("Group by"), "select").values(["day", "week"]))
                .act(READ),
        ))
    }

    async fn act(&self, _cx: &AbilityCx<'_>, request: &Request) -> Result<Outcome, AbilityError> {
        if request.act == SHOW {
            return Ok(Outcome::new(t!("Opened the overview.")).go("/admin/acme/overview"));
        }
        let from = request.set["range"]["from"]
            .as_str()
            .ok_or_else(|| AbilityError::invalid("set/range: give `from` and `to`"))?;
        Ok(Outcome::new(t!("The leads by day.")).data(json!({
            "from": from,
            "group_by": request.text("group_by"),
            "points": [3, 5, 8],
        })))
    }
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

#[tokio::test]
async fn a_component_of_a_modules_own_is_described_and_driven_by_its_parts() {
    let (ctx, _auth, id, _guard) = boot().await;
    let abilities = ctx.abilities();

    // Without the screen's permission it is not there, and with no screen
    // open the screen abilities are not offered at all.
    let outsider = person(id, &[]);
    assert!(abilities.get(&outsider, "backend.screens.list").is_none());

    let analyst = person(id, &["acme.crm.reports"]);
    let described = abilities
        .run(
            &analyst,
            "backend.screens.describe",
            json!({ "screen": "/acme/overview" }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    let chart = &described["parts"][0];
    assert_eq!(chart["name"], "leads_by_day");
    assert_eq!(chart["controls"][0]["kind"], "date_range");
    assert_eq!(chart["controls"][0]["value"]["from"], "2026-10-01");
    assert_eq!(chart["controls"][1]["values"], json!(["day", "week"]));
    assert_eq!(chart["acts"], json!(["read"]));

    let read = abilities
        .run(
            &analyst,
            "backend.screens.act",
            json!({
                "screen": "/acme/overview",
                "part": "leads_by_day",
                "act": "read",
                "set": { "range": { "from": "2026-11-01", "to": "2026-11-15" }, "group_by": "day" }
            }),
            Some("acme.assistant"),
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(read["from"], "2026-11-01");
    assert_eq!(read["points"], json!([3, 5, 8]));

    // The framework refuses what the component did not say it has, before
    // the component is asked.
    for (input, fault) in [
        (json!({ "part": "leads_by_week", "act": "read" }), "part"),
        (json!({ "part": "leads_by_day", "act": "show" }), "act"),
        (
            json!({ "part": "leads_by_day", "act": "read", "set": { "colour": "red" } }),
            "set/colour",
        ),
    ] {
        let mut input = input;
        input["screen"] = json!("/acme/overview");
        let refused = abilities
            .run(&analyst, "backend.screens.act", input, None)
            .await;
        let Err(AbilityError::Invalid(faults)) = refused else {
            panic!("expected a refusal for {fault}");
        };
        assert!(faults[0].starts_with(fault), "{faults:?}");
    }
}

/// A chart of a board: a metric and a number of days as settings, and the
/// board's own period.
struct Trend;

#[async_trait]
impl laterite_admin::board::Kind for Trend {
    fn code(&self) -> &str {
        "acme.trend"
    }

    fn label(&self) -> laterite_core::Text {
        t!("Trend")
    }

    fn description(&self) -> Option<laterite_core::Text> {
        Some(t!("One figure, day by day."))
    }

    fn properties(&self) -> Vec<laterite_admin::form::FormField> {
        use laterite_admin::form::FormField;
        vec![
            FormField::select(
                "metric",
                "Metric",
                vec![("signups", "Sign-ups"), ("orders", "Orders")],
            ),
            FormField::of("days", "Days", "number"),
        ]
    }

    fn check(&self, properties: &Value) -> Result<(), String> {
        match properties["metric"].as_str() {
            Some("signups" | "orders") => Ok(()),
            _ => Err("metric: choose sign-ups or orders".to_string()),
        }
    }

    fn reads(&self) -> bool {
        true
    }

    async fn data(
        &self,
        cx: &laterite_admin::board::ReadCx<'_>,
        properties: &Value,
    ) -> Result<Value, String> {
        Ok(json!({
            "metric": properties["metric"],
            "days": properties["days"],
            "from": cx.query.get("from"),
            "points": [4, 6, 9],
        }))
    }

    async fn render(
        &self,
        _cx: &laterite_admin::board::BoardCx<'_>,
        _properties: &Value,
    ) -> Result<laterite_admin::html::Markup, String> {
        Err("not drawn here".to_string())
    }
}

#[tokio::test]
async fn a_boards_widgets_are_parts_whose_settings_are_controls() {
    use laterite_admin::board::{Board, Layout, Placement};

    let (ctx, _auth, id, _guard) = boot().await;
    let board = Arc::new(
        Board::new(
            "acme.sales",
            Layout::rows(vec![vec![Placement::new("trend", "acme.trend", 10)
                .properties(json!({ "metric": "signups", "days": 30 }))]]),
        )
        .carry(["from", "to"])
        .with_kind(Arc::new(Trend)),
    );
    // A screen says so as it mounts.
    ctx.add_surface(board.surface("/acme/sales", t!("Sales")));
    let user = person(id, &[]);
    let abilities = ctx.abilities();

    let described = abilities
        .run(
            &user,
            "backend.screens.describe",
            json!({ "screen": "/acme/sales" }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(described["acts"], json!(["show"]));
    assert_eq!(described["controls"][0]["name"], "from");
    let trend = &described["parts"][0];
    assert_eq!(trend["name"], "trend");
    assert_eq!(trend["describe"], "One figure, day by day.");
    let controls: Vec<&str> = trend["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(controls, ["metric", "days", "from", "to"]);
    assert_eq!(trend["controls"][0]["values"], json!(["signups", "orders"]));
    assert_eq!(trend["controls"][0]["value"], "signups");
    assert_eq!(trend["acts"], json!(["read"]));

    // Read with a setting and the period changed; nothing is saved.
    let read = abilities
        .run(
            &user,
            "backend.screens.act",
            json!({
                "screen": "/acme/sales",
                "part": "trend",
                "act": "read",
                "set": { "metric": "orders", "from": "2026-11-01" }
            }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(read["metric"], "orders");
    assert_eq!(read["days"], 30);
    assert_eq!(read["from"], "2026-11-01");
    assert_eq!(read["points"], json!([4, 6, 9]));

    // The widget's own check answers for a value it does not take.
    let refused = abilities
        .run(
            &user,
            "backend.screens.act",
            json!({ "screen": "/acme/sales", "part": "trend", "act": "read", "set": { "metric": "refunds" } }),
            None,
        )
        .await;
    let Err(AbilityError::Invalid(faults)) = refused else {
        panic!("expected a refusal");
    };
    assert!(faults[0].starts_with("set: metric"), "{faults:?}");

    let shown = abilities
        .run(
            &user,
            "backend.screens.act",
            json!({ "screen": "/acme/sales", "act": "show", "set": { "from": "2026-11-01", "to": "2026-11-15" } }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        shown.go.as_deref(),
        Some("/admin/acme/sales?from=2026-11-01&to=2026-11-15")
    );
}
