//! The abilities every resource gives: kinds, find, read, and a drafted
//! create or update. Imports only the public API.

use std::sync::{Arc, OnceLock};

use axum::Router;
use laterite_admin::ability::AbilityError;
use laterite_admin::routes::{RouteCtx, Screen, ScreenReg};
use laterite_admin::{resource, router, AdminConfig, Contributions, Permission};
use laterite_auth::{
    AuditListener, AuthConfig, AuthService, AuthenticatedUser, BackendUser, NewOperator,
    PermissionSet,
};
use laterite_core::serde_json::json;
use laterite_core::strata::*;
use laterite_core::{t, CatalogStore, Db, MigrationSet, ModelListenerReg};
use sea_query::Alias;

struct CreatePosts;

#[async_trait(?Send)]
impl Migration for CreatePosts {
    fn name(&self) -> &str {
        "0001_create_posts"
    }
    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
        s.exec(
            Table::create()
                .table(Alias::new("posts"))
                .if_not_exists()
                .col(
                    ColumnDef::new(Alias::new("id"))
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
                .col(ColumnDef::new(Alias::new("code")).text().not_null())
                .col(ColumnDef::new(Alias::new("name")).text().not_null())
                .col(ColumnDef::new(Alias::new("internal")).text())
                .to_owned(),
        )
        .await
    }
}

struct Keeper(Arc<OnceLock<RouteCtx>>);

impl Screen for Keeper {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let _ = self.0.set(ctx.clone());
        Router::new()
    }
}

async fn boot() -> (
    RouteCtx,
    AuthService,
    i64,
    laterite_core::testing::TestGuard,
) {
    let mut sets = laterite_admin::builtin_migrations();
    sets.push(MigrationSet::new("acme.posts", vec![Box::new(CreatePosts)]));
    let (db, guard): (Db, _) = laterite_core::testing::connect_test(&sets).await;
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
    let kept = Arc::new(OnceLock::new());
    let _ = router(
        auth.clone(),
        db,
        Contributions {
            resources: vec![
                resource!("tests/descriptors/record_abilities.yaml"),
                resource!("tests/descriptors/record_abilities_off.yaml"),
            ],
            permissions: vec![
                Permission::new("acme.manage_posts", t!("Manage posts"), t!("Acme")),
                Permission::new("acme.edit_posts", t!("Edit posts"), t!("Acme")),
                Permission::new("acme.audit_posts", t!("Audit posts"), t!("Acme")),
            ],
            listeners: vec![ModelListenerReg::all(Arc::new(AuditListener))],
            screens: vec![ScreenReg::new(
                "/keeper",
                "acme.manage_posts",
                Arc::new(Keeper(kept.clone())),
            )],
            ..Default::default()
        },
        AdminConfig::default(),
        Arc::new(CatalogStore::default()),
    );
    let ctx = kept.get().expect("the screen mounted").clone();
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
        permissions: PermissionSet::new(false, grants.iter().map(|g| String::from(*g))),
    }
}

const EDITOR: [&str; 2] = ["acme.manage_posts", "acme.edit_posts"];

async fn add(ctx: &RouteCtx, user: &AuthenticatedUser, code: &str, name: &str) -> i64 {
    let outcome = ctx
        .abilities()
        .run(
            user,
            "backend.records.create",
            json!({ "resource": "/posts", "values": { "code": code, "name": name } }),
            Some("acme.assistant"),
        )
        .await
        .unwrap();
    outcome.data.unwrap()["id"].as_i64().unwrap()
}

#[tokio::test]
async fn nobody_without_a_resource_is_offered_the_record_abilities() {
    let (ctx, _auth, id, _guard) = boot().await;
    // The home dashboard is a screen everyone has, so the screen abilities
    // are there; the record abilities are not.
    let bare: Vec<String> = ctx
        .abilities()
        .for_user(&person(id, &[]))
        .iter()
        .map(|a| a.code().to_string())
        .collect();
    assert_eq!(
        bare,
        [
            "backend.screens.list",
            "backend.screens.describe",
            "backend.screens.act"
        ]
    );
    let codes: Vec<&str> = ctx
        .abilities()
        .for_user(&person(id, &["acme.manage_posts"]))
        .iter()
        .map(|a| a.code())
        .collect();
    assert_eq!(
        codes,
        [
            "backend.records.kinds",
            "backend.records.find",
            "backend.records.open",
            "backend.records.read",
            "backend.records.create",
            "backend.records.update",
            "backend.records.delete",
            "backend.screens.list",
            "backend.screens.describe",
            "backend.screens.act"
        ]
    );
}

#[tokio::test]
async fn kinds_lists_what_is_open_and_hides_what_the_resource_hides() {
    let (ctx, _auth, id, _guard) = boot().await;
    let reader = person(id, &["acme.manage_posts"]);
    let abilities = ctx.abilities();

    let all = abilities
        .run(&reader, "backend.records.kinds", json!({}), None)
        .await
        .unwrap()
        .data
        .unwrap();
    // The resource that switched abilities off is not there.
    assert_eq!(all.as_array().unwrap().len(), 1);
    assert_eq!(all[0]["resource"], "/posts");
    assert_eq!(
        all[0]["describe"],
        "Blog posts, each with a code and a name."
    );
    // Without the update grant the person may add and not change.
    assert_eq!(all[0]["can"], json!(["find", "read", "create", "delete"]));

    let one = abilities
        .run(
            &reader,
            "backend.records.kinds",
            json!({ "resource": "/posts" }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    let names: Vec<&str> = one["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["code", "name"]);
    assert_eq!(one["fields"][0]["required"], true);
    assert_eq!(one["columns"], json!(["code", "name"]));

    let sealed = abilities
        .run(
            &reader,
            "backend.records.find",
            json!({ "resource": "/sealed" }),
            None,
        )
        .await;
    assert!(matches!(sealed, Err(AbilityError::Invalid(_))));
}

#[tokio::test]
async fn a_record_is_created_found_read_and_changed_as_the_person() {
    let (ctx, auth, id, _guard) = boot().await;
    let editor = person(id, &EDITOR);
    let abilities = ctx.abilities();

    let first = add(&ctx, &editor, "hello", "Hello world").await;
    add(&ctx, &editor, "other", "Something else").await;

    let found = abilities
        .run(
            &editor,
            "backend.records.find",
            json!({ "resource": "/posts", "search": "hello" }),
            None,
        )
        .await
        .unwrap();
    let data = found.data.unwrap();
    assert_eq!(data["total"], 1);
    assert_eq!(data["rows"][0]["code"], "hello");
    assert!(data["rows"][0].get("internal").is_none());
    assert_eq!(found.links[0].href, format!("/admin/posts/{first}/edit"));

    let changed = abilities
        .run(
            &editor,
            "backend.records.update",
            json!({ "resource": "/posts", "id": first, "values": { "name": "Hello again" } }),
            Some("acme.assistant"),
        )
        .await
        .unwrap();
    assert_eq!(changed.data.unwrap()["id"], first);

    let read = abilities
        .run(
            &editor,
            "backend.records.read",
            json!({ "resource": "/posts", "id": first }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    // The field left out of the change kept its value.
    assert_eq!(
        read["values"],
        json!({ "code": "hello", "name": "Hello again" })
    );

    // Each save is in the trail as the person, through the channel.
    let entries = auth.recent_audit(20).await.unwrap();
    let saves: Vec<&str> = entries
        .iter()
        .filter(|e| e.target_type.as_deref() == Some("posts"))
        .map(|e| e.action.as_str())
        .collect();
    assert_eq!(
        saves,
        [
            "backend.posts.update",
            "backend.posts.create",
            "backend.posts.create"
        ]
    );
    assert!(entries
        .iter()
        .all(|e| e.actor_user_id == Some(id) && e.channel.as_deref() == Some("acme.assistant")));
}

#[tokio::test]
async fn a_draft_is_checked_and_handed_back_with_nothing_written() {
    let (ctx, auth, id, _guard) = boot().await;
    let editor = person(id, &EDITOR);
    let abilities = ctx.abilities();

    let preview = abilities
        .preview(
            &editor,
            "backend.records.create",
            &json!({ "resource": "/posts", "values": { "code": "draft", "name": "A draft" } }),
            None,
        )
        .await
        .unwrap();
    let draft = preview.draft.unwrap();
    assert_eq!(draft.path, "/posts");
    assert_eq!(draft.id, None);
    assert_eq!(draft.values["name"], "A draft");

    // The form's own rules answer, against the field.
    let refused = abilities
        .preview(
            &editor,
            "backend.records.create",
            &json!({ "resource": "/posts", "values": { "code": "x".repeat(61) } }),
            None,
        )
        .await;
    let Err(AbilityError::Invalid(faults)) = refused else {
        panic!("expected a refusal");
    };
    assert!(
        faults.iter().any(|f| f.starts_with("values/code")),
        "{faults:?}"
    );
    assert!(
        faults.iter().any(|f| f.starts_with("values/name")),
        "{faults:?}"
    );

    // A hidden field is never written.
    let hidden = abilities
        .run(
            &editor,
            "backend.records.create",
            json!({ "resource": "/posts", "values": { "code": "a", "name": "b", "internal": "c" } }),
            None,
        )
        .await;
    let Err(AbilityError::Invalid(faults)) = hidden else {
        panic!("expected a refusal");
    };
    assert_eq!(
        faults,
        ["values/internal: this kind of record has no such field"]
    );
    assert!(auth.recent_audit(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_change_asks_for_the_resources_own_update_grant() {
    let (ctx, _auth, id, _guard) = boot().await;
    let first = add(&ctx, &person(id, &EDITOR), "hello", "Hello world").await;

    let reader = person(id, &["acme.manage_posts"]);
    let refused = ctx
        .abilities()
        .run(
            &reader,
            "backend.records.update",
            json!({ "resource": "/posts", "id": first, "values": { "name": "Mine now" } }),
            None,
        )
        .await;
    assert!(matches!(refused, Err(AbilityError::Refused(_))));
    let missing = ctx
        .abilities()
        .run(
            &person(id, &EDITOR),
            "backend.records.read",
            json!({ "resource": "/posts", "id": 999 }),
            None,
        )
        .await;
    assert!(matches!(missing, Err(AbilityError::Refused(_))));
}

#[tokio::test]
async fn a_list_says_its_filters_and_is_found_and_opened_through_them() {
    let (ctx, _auth, id, _guard) = boot().await;
    let writer = person(id, &["acme.manage_posts"]);
    let abilities = ctx.abilities();
    add(&ctx, &writer, "a", "First post").await;
    add(&ctx, &writer, "b", "Second post").await;

    let kind = abilities
        .run(
            &writer,
            "backend.records.kinds",
            json!({ "resource": "/posts" }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    // The filter behind a permission is not named to someone without it.
    let names: Vec<&str> = kind["filters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["name", "code"]);
    assert_eq!(kind["filters"][1]["values"], json!(["a", "b"]));
    assert_eq!(kind["sorts"], json!(["code", "name"]));

    let found = abilities
        .run(
            &writer,
            "backend.records.find",
            json!({ "resource": "/posts", "filters": { "code": "b" }, "sort": "name", "dir": "desc" }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(found["total"], 1);
    assert_eq!(found["rows"][0]["name"], "Second post");

    let opened = abilities
        .run(
            &writer,
            "backend.records.open",
            json!({
                "resource": "/posts",
                "search": "post & more",
                "filters": { "name": "second", "code": "b" },
                "sort": "name"
            }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        opened.go.as_deref(),
        Some("/admin/posts?q=post+%26+more&f_name=second&f_code=b&sort=name&dir=asc")
    );

    // What the list does not offer is named, and nothing is opened.
    for (input, fault) in [
        (json!({ "filters": { "audit": "x" } }), "filters/audit"),
        (json!({ "filters": { "code": "z" } }), "filters/code"),
        (json!({ "sort": "internal" }), "sort"),
    ] {
        let mut input = input;
        input["resource"] = json!("/posts");
        let refused = abilities
            .run(&writer, "backend.records.open", input, None)
            .await;
        let Err(AbilityError::Invalid(faults)) = refused else {
            panic!("expected a refusal for {fault}");
        };
        assert!(faults[0].starts_with(fault), "{faults:?}");
    }

    let auditor = person(id, &["acme.manage_posts", "acme.audit_posts"]);
    let opened = abilities
        .run(
            &auditor,
            "backend.records.open",
            json!({ "resource": "/posts", "filters": { "audit": "x" } }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.go.as_deref(), Some("/admin/posts?f_audit=x"));
}

#[tokio::test]
async fn a_list_is_a_screen_described_and_driven_like_any_other() {
    let (ctx, _auth, id, _guard) = boot().await;
    let writer = person(id, &["acme.manage_posts"]);
    let abilities = ctx.abilities();
    add(&ctx, &writer, "a", "First post").await;
    add(&ctx, &writer, "b", "Second post").await;

    let screens = abilities
        .run(&writer, "backend.screens.list", json!({}), None)
        .await
        .unwrap()
        .data
        .unwrap();
    // Their own preferences page, the home dashboard, then the list.
    let named: Vec<&str> = screens
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["screen"].as_str().unwrap())
        .collect();
    assert_eq!(named, ["/preferences/backend/my_abilities", "/", "/posts"]);

    let described = abilities
        .run(
            &writer,
            "backend.screens.describe",
            json!({ "screen": "/posts" }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    let controls: Vec<&str> = described["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(controls, ["search", "name", "code", "sort", "dir"]);
    assert_eq!(described["controls"][2]["values"], json!(["a", "b"]));
    assert_eq!(described["acts"], json!(["show", "read"]));

    let shown = abilities
        .run(
            &writer,
            "backend.screens.act",
            json!({ "screen": "/posts", "act": "show", "set": { "code": "b", "search": "post" } }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(shown.go.as_deref(), Some("/admin/posts?q=post&f_code=b"));

    let read = abilities
        .run(
            &writer,
            "backend.screens.act",
            json!({ "screen": "/posts", "act": "read", "set": { "code": "b" } }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(read["total"], 1);

    for (input, fault) in [
        (
            json!({ "act": "show", "set": { "audit": "x" } }),
            "set/audit",
        ),
        (json!({ "act": "show", "part": "rows" }), "part"),
        (json!({ "act": "delete" }), "act"),
    ] {
        let mut input = input;
        input["screen"] = json!("/posts");
        let refused = abilities
            .run(&writer, "backend.screens.act", input, None)
            .await;
        let Err(AbilityError::Invalid(faults)) = refused else {
            panic!("expected a refusal for {fault}");
        };
        assert!(faults[0].starts_with(fault), "{faults:?}");
    }
}

#[tokio::test]
async fn a_record_is_deleted_only_once_an_administrator_allows_it() {
    use laterite_admin::ability::{Confirm, PanelPolicy, PolicyRule};

    let (ctx, auth, id, _guard) = boot().await;
    let editor = person(id, &EDITOR);
    let abilities = ctx.abilities();
    let post = add(&ctx, &editor, "hello", "Hello world").await;
    let input = json!({ "resource": "/posts", "id": post });

    // Out of the box a channel is not offered deleting at all.
    let refused = abilities
        .run(
            &editor,
            "backend.records.delete",
            input.clone(),
            Some("acme.assistant"),
        )
        .await
        .unwrap_err();
    assert!(matches!(refused, AbilityError::Unknown(_)), "{refused:?}");

    laterite_admin::settings::store::save(
        ctx.db(),
        &PanelPolicy {
            rules: vec![PolicyRule::new("backend.records.delete", Confirm::Ask)],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        abilities
            .confirm(&editor, "backend.records.delete")
            .await
            .unwrap(),
        Confirm::Ask
    );

    // The preview names the record and writes nothing.
    let preview = abilities
        .preview(
            &editor,
            "backend.records.delete",
            &input,
            Some("acme.assistant"),
        )
        .await
        .unwrap();
    assert_eq!(preview.count, Some(1));
    assert_eq!(preview.records[0].href, format!("/admin/posts/{post}/edit"));

    let gone = abilities
        .run(
            &editor,
            "backend.records.delete",
            input.clone(),
            Some("acme.assistant"),
        )
        .await
        .unwrap();
    assert_eq!(gone.data.unwrap()["id"], post);
    let again = abilities
        .run(
            &editor,
            "backend.records.delete",
            input,
            Some("acme.assistant"),
        )
        .await
        .unwrap_err();
    assert!(matches!(again, AbilityError::Refused(_)), "{again:?}");

    // The delete is in the trail as the person, through the channel.
    let entries = auth.recent_audit(20).await.unwrap();
    assert!(entries.iter().any(|e| e.action == "backend.posts.delete"
        && e.actor_user_id == Some(id)
        && e.channel.as_deref() == Some("acme.assistant")));
}

#[tokio::test]
async fn a_settings_page_says_what_it_holds_and_is_changed_only_on_its_page() {
    use laterite_admin::ability::{Confirm, PanelPolicy, PolicyRule};

    let (ctx, _auth, id, _guard) = boot().await;
    let abilities = ctx.abilities();
    laterite_admin::settings::store::save(
        ctx.db(),
        &PanelPolicy {
            rules: vec![PolicyRule::new("effect:delete", Confirm::Ask)],
        },
    )
    .await
    .unwrap();
    let page = "/settings/backend/abilities";

    // Without the page's permission it is not a screen at all.
    let outsider = person(id, &["acme.manage_posts"]);
    let hidden = abilities
        .run(
            &outsider,
            "backend.screens.describe",
            json!({ "screen": page }),
            None,
        )
        .await;
    assert!(matches!(hidden, Err(AbilityError::Invalid(_))));

    let admin = person(id, &["backend.manage_abilities"]);
    let described = abilities
        .run(
            &admin,
            "backend.screens.describe",
            json!({ "screen": page }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(described["label"], "Abilities");
    assert_eq!(described["controls"][0]["name"], "rules");
    assert_eq!(
        described["controls"][0]["value"][0]["what"],
        "effect:delete"
    );
    assert_eq!(described["acts"], json!(["show", "read"]));

    let read = abilities
        .run(
            &admin,
            "backend.screens.act",
            json!({ "screen": page, "act": "read" }),
            None,
        )
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(read["rules"][0]["confirm"], "ask");

    let shown = abilities
        .run(
            &admin,
            "backend.screens.act",
            json!({ "screen": page, "act": "show" }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        shown.go.as_deref(),
        Some("/admin/settings/backend/abilities")
    );

    let refused = abilities
        .run(
            &admin,
            "backend.screens.act",
            json!({ "screen": page, "act": "show", "set": { "rules": [] } }),
            None,
        )
        .await;
    let Err(AbilityError::Invalid(faults)) = refused else {
        panic!("expected a refusal");
    };
    assert!(faults[0].starts_with("set: a setting is changed on its page"));
}
