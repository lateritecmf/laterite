//! The palette's modes over real requests: which modes a person is offered,
//! and Do walking an ability through its form, its card and its result.
//! Imports only the public API.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use laterite_admin::ability::{
    Ability, AbilityCx, AbilityError, AbilityHandler, Effect, Outcome, Preview,
};
use laterite_admin::palette::PaletteMode;
use laterite_admin::{router, AdminConfig, AdminRegistry, Contributions, Permission};
use laterite_auth::{password, store, AuthConfig, AuthService, NewOperator, RequestContext};
use laterite_core::serde_json::{json, Value};
use laterite_core::strata::async_trait;
use laterite_core::{t, CatalogStore, Db, ModuleId, Registry};
use tower::ServiceExt;

const SESSION_COOKIE: &str = "laterite_session";

// Only its schema is read here.
#[allow(dead_code)]
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddLead {
    /// Their full name.
    name: String,
    /// How many seats they asked for.
    seats: Option<u32>,
}

struct AddLeadHandler;

#[async_trait]
impl AbilityHandler for AddLeadHandler {
    async fn preview(&self, _cx: &AbilityCx<'_>, input: &Value) -> Result<Preview, AbilityError> {
        Ok(Preview::new(t!(
            "Adds {name} to the leads list.",
            name = input["name"].as_str().unwrap_or_default()
        )))
    }

    async fn run(&self, _cx: &AbilityCx<'_>, input: Value) -> Result<Outcome, AbilityError> {
        if input["name"] == "Nobody" {
            return Err(AbilityError::Refused(t!("That person is already a lead.")));
        }
        Ok(Outcome::new(t!("Added the lead.")).data(input))
    }
}

struct CountHandler;

#[async_trait]
impl AbilityHandler for CountHandler {
    async fn run(&self, _cx: &AbilityCx<'_>, _input: Value) -> Result<Outcome, AbilityError> {
        Ok(Outcome::new(t!("There are 3 leads.")))
    }
}

async fn setup() -> (Db, laterite_core::testing::TestGuard, Router) {
    let (db, guard) =
        laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await;
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
        .effect(Effect::Create),
    );
    registry.add_ability(
        Ability::new(
            "acme.crm.count_leads",
            t!("Count the leads"),
            Arc::new(CountHandler),
        )
        .effect(Effect::Read),
    );
    // A nested input is for a program: Do leaves it out.
    registry.add_ability(
        Ability::new(
            "acme.crm.import_leads",
            t!("Import leads"),
            Arc::new(CountHandler),
        )
        .permission("acme.crm.leads.create")
        .input_schema(json!({
            "type": "object",
            "properties": { "rows": { "type": "array", "items": { "type": "object" } } }
        }))
        .effect(Effect::Create),
    );
    registry.add_palette_mode(
        PaletteMode::new("acme.assistant", t!("Ask"), "/acme/assistant/ask")
            .placeholder(t!("Ask anything"))
            .prefix('?')
            .permission("acme.assistant.use"),
    );

    let mut config = AdminConfig::default();
    config.origin = "https://acme.test".to_string();
    let app = router(
        AuthService::new(db.clone(), AuthConfig::default()),
        db.clone(),
        Contributions {
            permissions: vec![
                Permission::new("acme.crm.leads.create", t!("Add leads"), t!("Acme")),
                Permission::new("acme.assistant.use", t!("Use the assistant"), t!("Acme")),
            ],
            plugin_defined: Arc::new(registry),
            ..Default::default()
        },
        config,
        Arc::new(CatalogStore::default()),
    );
    (db, guard, app)
}

/// A session for an operator holding `grants`; a superuser when there are none.
async fn session(db: &Db, name: &str, grants: &[&str]) -> String {
    let svc = AuthService::new(db.clone(), AuthConfig::default());
    if grants.is_empty() {
        svc.create_superuser(NewOperator {
            username: name,
            email: &format!("{name}@acme.test"),
            first_name: name,
            last_name: None,
            password: "secretpw12345",
            timezone: None,
        })
        .await
        .unwrap();
    } else {
        let hash = password::hash_password("secretpw12345").unwrap();
        let user = store::create_user(
            db,
            name,
            &format!("{name}@acme.test"),
            name,
            None,
            &hash,
            false,
        )
        .await
        .unwrap();
        let grants: Vec<String> = grants.iter().map(|g| String::from(*g)).collect();
        let role = store::create_role(db, name, name, &grants).await.unwrap();
        store::assign_role(db, user, role).await.unwrap();
    }
    let token = svc
        .authenticate(name, "secretpw12345", &RequestContext::default())
        .await
        .unwrap()
        .token;
    svc.set_session_data(&token, r#"{"v":1,"csrf":"tok-123"}"#)
        .await
        .unwrap();
    token
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn get(path: &str, session: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, session: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("cookie", format!("{SESSION_COOKIE}={session}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "tok-123")
        .header("x-requested-with", "fetch")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn mode_codes(app: &Router, session: &str) -> Vec<String> {
    let (status, body) = send(app, get("/admin/palette/modes", session)).await;
    assert_eq!(status, StatusCode::OK);
    let modes: Value = serde_json::from_str(&body).unwrap();
    modes
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["code"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_person_is_offered_the_modes_their_permissions_allow() {
    let (db, _guard, app) = setup().await;

    let all = session(&db, "root", &[]).await;
    let (_, body) = send(&app, get("/admin/palette/modes", &all)).await;
    let modes: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(modes[0]["code"], "backend.do");
    assert_eq!(modes[0]["prefix"], ">");
    assert_eq!(modes[0]["url"], "/admin/palette/do");
    assert_eq!(modes[0]["live"], true);
    assert_eq!(modes[1]["code"], "acme.assistant");
    assert_eq!(modes[1]["label"], "Ask");
    assert_eq!(modes[1]["placeholder"], "Ask anything");
    assert_eq!(modes[1]["prefix"], "?");
    assert_eq!(modes[1]["url"], "/admin/acme/assistant/ask");
    assert_eq!(modes[1]["live"], false);

    // The read ability is open to everyone, so Do stays; the assistant goes.
    let plain = session(&db, "plain", &["acme.other"]).await;
    assert_eq!(mode_codes(&app, &plain).await, ["backend.do"]);

    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/admin/palette/modes")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "signed out");
}

#[tokio::test]
async fn do_lists_the_abilities_a_person_can_fill_by_hand() {
    let (db, _guard, app) = setup().await;
    let writer = session(&db, "writer", &["acme.crm.leads.create"]).await;

    let (status, html) = send(&app, post("/admin/palette/do", &writer, "q=lead")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Add a lead"), "{html}");
    assert!(html.contains("Count the leads"));
    assert!(html.contains(r#"hx-get="/admin/palette/do/acme.crm.add_lead""#));
    assert!(!html.contains("Import leads"), "a nested input is left out");
    assert!(!html.contains("backend.records"), "record abilities too");

    let plain = session(&db, "plain", &["acme.other"]).await;
    let (_, html) = send(&app, post("/admin/palette/do", &plain, "q=lead")).await;
    assert!(html.contains("Count the leads"));
    assert!(!html.contains("Add a lead"));

    let (_, html) = send(&app, post("/admin/palette/do", &writer, "q=zzzz")).await;
    assert!(html.contains("Nothing matches."));
}

#[tokio::test]
async fn a_change_asks_for_its_input_shows_a_card_then_runs() {
    let (db, _guard, app) = setup().await;
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    let writer = session(&db, "writer", &["acme.crm.leads.create"]).await;
    let base = "/admin/palette/do/acme.crm.add_lead";

    let (status, html) = send(&app, get(base, &writer)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-palette-raise"));
    assert!(html.contains(r#"name="in.name""#));
    assert!(html.contains("Their full name."));
    assert!(html.contains(r#"type="number" name="in.seats""#));

    // A value the schema refuses comes back on the form, still filled in.
    let (_, html) = send(
        &app,
        post(
            &format!("{base}/preview"),
            &writer,
            "in.name=Ravi&in.seats=-2",
        ),
    )
    .await;
    assert!(html.contains("lat-palette__faults"), "{html}");
    assert!(html.contains(r#"value="Ravi""#));

    let (_, html) = send(
        &app,
        post(
            &format!("{base}/preview"),
            &writer,
            "in.name=Ravi&in.seats=2",
        ),
    )
    .await;
    assert!(html.contains("Adds Ravi to the leads list."), "{html}");
    assert!(html.contains(&format!(r#"hx-post="{base}/run""#)));
    // Nothing has run yet.
    assert!(auth.recent_audit(10).await.unwrap().is_empty());

    let input = r#"{"name":"Ravi","seats":2}"#;
    let body = format!("input={}", urlencode(input));
    let (status, html) = send(&app, post(&format!("{base}/run"), &writer, &body)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Added the lead."), "{html}");
    let entries = auth.recent_audit(10).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, "ability.run");
    assert_eq!(entries[0].channel, None);

    // A refusal reads as one, and writes nothing more.
    let body = format!("input={}", urlencode(r#"{"name":"Nobody"}"#));
    let (_, html) = send(&app, post(&format!("{base}/run"), &writer, &body)).await;
    assert!(html.contains("That person is already a lead."));
    assert!(html.contains(r#"role="alert""#));
    assert_eq!(auth.recent_audit(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_read_runs_at_once_and_the_rest_is_refused() {
    let (db, _guard, app) = setup().await;
    let auth = AuthService::new(db.clone(), AuthConfig::default());
    let plain = session(&db, "plain", &["acme.other"]).await;

    let (status, html) = send(&app, get("/admin/palette/do/acme.crm.count_leads", &plain)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("There are 3 leads."));

    // Without the permission the ability does not exist, to see or to run.
    let base = "/admin/palette/do/acme.crm.add_lead";
    let (status, _) = send(&app, get(base, &plain)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body = format!("input={}", urlencode(r#"{"name":"Ravi"}"#));
    let (status, _) = send(&app, post(&format!("{base}/run"), &plain, &body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Nor does one only a program can call, or a record ability.
    let root = session(&db, "root", &[]).await;
    for code in ["acme.crm.import_leads", "backend.records.kinds"] {
        let (status, _) = send(&app, get(&format!("/admin/palette/do/{code}"), &root)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{code}");
    }

    // A post without the token is refused before anything runs.
    let (status, _) = send(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("{base}/run"))
            .header("cookie", format!("{SESSION_COOKIE}={root}"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(auth.recent_audit(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_rules_over_abilities_are_set_from_settings_and_preferences() {
    use laterite_admin::ability::{Confirm, OwnPolicy, PanelPolicy};
    use laterite_admin::settings::store as settings;

    let (db, _guard, app) = setup().await;
    let root = session(&db, "root", &[]).await;
    let plain = session(&db, "plain", &["acme.other"]).await;

    let (status, html) = send(&app, get("/admin/settings/backend/abilities", &root)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Add a lead (acme.crm.add_lead)"), "{html}");
    assert!(html.contains("All of acme.crm"));
    assert!(html.contains("Everything that deletes a record"));
    assert!(html.contains("Run without asking"));
    let (status, _) = send(&app, get("/admin/settings/backend/abilities", &plain)).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "the panel's rules need their permission"
    );

    let body = format!(
        "{}=acme.crm.add_lead&{}=auto",
        urlencode("rules[0][what]"),
        urlencode("rules[0][confirm]")
    );
    let (status, _) = send(
        &app,
        post("/admin/settings/backend/abilities", &root, &body),
    )
    .await;
    assert!(status.is_redirection() || status.is_success(), "{status}");
    let panel: PanelPolicy = settings::load(&db).await.unwrap();
    assert_eq!(panel.rules.len(), 1);
    assert_eq!(panel.rules[0].what, "acme.crm.add_lead");
    assert_eq!(panel.rules[0].confirm, Confirm::Auto);

    // Anyone signed in sets their own.
    let path = "/admin/preferences/backend/my_abilities";
    let (status, html) = send(&app, get(path, &plain)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Ask first"));
    let body = format!(
        "{}={}&{}=off",
        urlencode("rules[0][what]"),
        urlencode("effect:read"),
        urlencode("rules[0][confirm]")
    );
    let (status, _) = send(&app, post(path, &plain, &body)).await;
    assert!(status.is_redirection() || status.is_success(), "{status}");
    let user = store_user(&db, "plain").await;
    let own: OwnPolicy = settings::load_for(&db, user).await.unwrap();
    assert_eq!(own.rules[0].what, "effect:read");
    assert_eq!(own.rules[0].confirm, Confirm::Off);
}

/// The id of the operator called `name`.
async fn store_user(db: &Db, name: &str) -> i64 {
    store::find_user_by_username(db, name)
        .await
        .unwrap()
        .unwrap()
        .id
}

/// Percent-encodes a form value.
fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
