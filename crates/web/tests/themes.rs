//! The theme store: rows, the parent chain, revisions, locks and the active
//! and edit theme. Imports only the public API.

use laterite_core::{Db, Module};
use laterite_web::theme::{self, store, Kind, ThemeError, ThemeModule};

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

const BASE: &str = "name: Base\nauthor: Acme\nhomepage: https://acme.example\n";

#[tokio::test]
async fn a_theme_is_created_from_its_config_and_listed() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", BASE).await.unwrap();
    assert_eq!(base.config.name, "Base");
    assert_eq!(base.config.author, "Acme");
    assert!(!base.locked);
    assert_eq!(base.config_source, BASE);
    let all = store::list_themes(&db).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].code, "base");

    assert!(matches!(
        store::create_theme(&db, "Base", BASE).await,
        Err(ThemeError::Code(_))
    ));
    assert!(matches!(
        store::create_theme(&db, "base", BASE).await,
        Err(ThemeError::Exists(_))
    ));
    assert!(matches!(
        store::create_theme(&db, "broken", "name: X\nunknown: 1\n").await,
        Err(ThemeError::Config(_))
    ));
}

#[tokio::test]
async fn a_child_resolves_through_its_parent_and_overrides_it() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", BASE).await.unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Layout,
        "default.htm",
        None,
        "base layout",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Partial,
        "nav.htm",
        None,
        "base nav",
        None,
    )
    .await
    .unwrap();
    let child = store::create_child_theme(&db, "base", "child", "Child")
        .await
        .unwrap();
    assert_eq!(child.parent.as_deref(), Some("base"));
    assert_eq!(child.config.name, "Child");
    assert_eq!(child.config.author, "Acme");

    let (from, t) = store::resolve_template(&db, "child", Kind::Layout, "default.htm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(from.code, "base");
    assert_eq!(t.markup, "base layout");

    store::save_template(
        &db,
        child.id,
        Kind::Partial,
        "nav.htm",
        None,
        "child nav",
        None,
    )
    .await
    .unwrap();
    let (from, t) = store::resolve_template(&db, "child", Kind::Partial, "nav.htm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(from.code, "child");
    assert_eq!(t.markup, "child nav");
    assert!(
        store::resolve_template(&db, "child", Kind::Page, "missing.htm")
            .await
            .unwrap()
            .is_none()
    );

    // A parent that leads back is refused, as is a missing one.
    assert!(matches!(
        store::update_theme_config(&db, "base", "name: Base\nparent: child\n").await,
        Err(ThemeError::Parent(_))
    ));
    assert!(matches!(
        store::update_theme_config(&db, "base", "name: Base\nparent: nowhere\n").await,
        Err(ThemeError::Parent(_))
    ));
    // The parent cannot go while the child names it.
    assert!(matches!(
        store::delete_theme(&db, "base").await,
        Err(ThemeError::Parent(_))
    ));
    store::delete_theme(&db, "child").await.unwrap();
    store::delete_theme(&db, "base").await.unwrap();
    assert!(store::list_themes(&db).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_save_keeps_a_revision_and_a_revision_restores() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", BASE).await.unwrap();
    let first = store::save_template(
        &db,
        base.id,
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/"})),
        "v1",
        Some(7),
    )
    .await
    .unwrap();
    assert!(store::revisions(&db, first.id).await.unwrap().is_empty());

    let second = store::save_template(&db, base.id, Kind::Page, "home.htm", None, "v2", Some(7))
        .await
        .unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.markup, "v2");
    assert_eq!(second.settings, serde_json::json!({"url": "/"}));
    let revs = store::revisions(&db, first.id).await.unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].markup, "v1");
    assert_eq!(revs[0].saved_by, Some(7));

    let restored = store::restore_revision(&db, revs[0].id, Some(8))
        .await
        .unwrap();
    assert_eq!(restored.markup, "v1");
    let revs = store::revisions(&db, first.id).await.unwrap();
    assert_eq!(revs.len(), 2);
    assert_eq!(revs[0].markup, "v2");

    assert!(matches!(
        store::save_template(&db, base.id, Kind::Page, "../x.htm", None, "", None).await,
        Err(ThemeError::Path(_))
    ));
    store::delete_template(&db, first.id).await.unwrap();
    assert!(store::template(&db, first.id).await.unwrap().is_none());
    assert!(store::revisions(&db, first.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_locked_theme_refuses_writes_and_a_copy_is_unlocked() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", BASE).await.unwrap();
    let t = store::save_template(&db, base.id, Kind::Layout, "default.htm", None, "x", None)
        .await
        .unwrap();
    store::set_locked(&db, "base", true).await.unwrap();
    assert!(matches!(
        store::save_template(&db, base.id, Kind::Layout, "default.htm", None, "y", None).await,
        Err(ThemeError::Locked(_))
    ));
    assert!(matches!(
        store::delete_template(&db, t.id).await,
        Err(ThemeError::Locked(_))
    ));
    assert!(matches!(
        store::delete_theme(&db, "base").await,
        Err(ThemeError::Locked(_))
    ));

    let copy = store::duplicate_theme(&db, "base", "copy").await.unwrap();
    assert!(!copy.locked);
    assert_eq!(copy.config.name, "Base");
    let copied = store::list_templates(&db, copy.id, None).await.unwrap();
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].path, "default.htm");
    assert_eq!(copied[0].markup, "x");
    assert_eq!(
        store::list_templates(&db, copy.id, Some(Kind::Page))
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn the_active_theme_and_an_operators_edit_theme() {
    let (db, _g) = test_db().await;
    assert!(theme::active_theme(&db).await.unwrap().is_none());
    assert!(matches!(
        theme::set_active_theme(&db, "base").await,
        Err(ThemeError::NotFound(_))
    ));
    store::create_theme(&db, "base", BASE).await.unwrap();
    store::create_theme(&db, "next", "name: Next\n")
        .await
        .unwrap();
    theme::set_active_theme(&db, "base").await.unwrap();
    assert_eq!(
        theme::active_theme(&db).await.unwrap().unwrap().code,
        "base"
    );
    assert!(matches!(
        store::delete_theme(&db, "base").await,
        Err(ThemeError::Active(_))
    ));

    let svc = laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default());
    let user_id = svc
        .create_superuser(laterite_auth::NewOperator {
            username: "root",
            email: "root@acme.test",
            first_name: "Root",
            last_name: None,
            password: "rootpw12345",
            timezone: None,
        })
        .await
        .unwrap();
    assert_eq!(
        theme::edit_theme(&db, user_id).await.unwrap().unwrap().code,
        "base"
    );
    theme::set_edit_theme(&db, user_id, Some("next"))
        .await
        .unwrap();
    assert_eq!(
        theme::edit_theme(&db, user_id).await.unwrap().unwrap().code,
        "next"
    );
    // A deleted edit theme falls back to the active one.
    store::delete_theme(&db, "next").await.unwrap();
    assert_eq!(
        theme::edit_theme(&db, user_id).await.unwrap().unwrap().code,
        "base"
    );
    theme::set_edit_theme(&db, user_id, None).await.unwrap();
    assert_eq!(
        theme::edit_theme(&db, user_id).await.unwrap().unwrap().code,
        "base"
    );
}

#[tokio::test]
async fn settings_are_saved_per_theme_and_the_form_falls_through() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(
        &db,
        "base",
        "name: Base\nform:\n  fields:\n    site_name:\n      label: Site name\n      type: text\n",
    )
    .await
    .unwrap();
    assert!(base.settings.as_object().unwrap().is_empty());
    store::create_child_theme(&db, "base", "child", "Child")
        .await
        .unwrap();
    let chain = store::chain(&db, "child").await.unwrap();
    let fields = store::form_fields(&chain);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "site_name");

    store::save_settings(&db, "child", &serde_json::json!({"site_name": "Acme"}))
        .await
        .unwrap();
    let child = store::find_theme(&db, "child").await.unwrap().unwrap();
    assert_eq!(child.settings, serde_json::json!({"site_name": "Acme"}));
    let base = store::find_theme(&db, "base").await.unwrap().unwrap();
    assert!(
        base.settings.as_object().unwrap().is_empty(),
        "values are per theme"
    );

    assert!(matches!(
        store::save_settings(&db, "child", &serde_json::json!([1])).await,
        Err(ThemeError::Config(_))
    ));
    // A field cannot take a key the theme value owns.
    assert!(matches!(
        store::create_theme(
            &db,
            "clash",
            "name: X\nform:\n  fields:\n    name:\n      label: Name\n      type: text\n",
        )
        .await,
        Err(ThemeError::Config(_))
    ));
}
