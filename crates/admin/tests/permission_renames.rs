//! A renamed permission repairs what operators saved: their own roles and
//! their per-user overrides, not just the framework's roles.

use laterite_admin::{permission_renames, system_roles, Permission, ROLE_EDITOR};
use laterite_auth::{password, store};
use laterite_core::{t, Db};

const OLD: &str = "acme.manage.posts";
const NEW: &str = "acme.posts";

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    laterite_core::testing::connect_test(&laterite_admin::builtin_migrations()).await
}

fn registry() -> Vec<Permission> {
    vec![Permission::new(NEW, t!("Manage posts"), t!("Acme"))
        .roles([ROLE_EDITOR])
        .renamed_from([OLD])]
}

async fn a_user(db: &Db, name: &str) -> i64 {
    let hash = password::hash_password("operatorpw12345").unwrap();
    store::create_user(
        db,
        name,
        &format!("{name}@acme.test"),
        name,
        None,
        &hash,
        false,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_role_someone_made_is_rewritten() {
    let (db, _guard) = test_db().await;
    let mine = store::create_role(
        &db,
        "acme.mine",
        "Mine",
        &[OLD.to_string(), "acme.other".into()],
    )
    .await
    .unwrap();

    let changed = store::rename_permissions(&db, &permission_renames(&registry()))
        .await
        .unwrap();
    assert!(changed >= 1);
    assert_eq!(
        store::role_permissions(&db, mine).await.unwrap(),
        [NEW, "acme.other"],
        "the old code became the new one, in place"
    );
}

#[tokio::test]
async fn a_per_user_override_is_rewritten() {
    let (db, _guard) = test_db().await;
    let id = a_user(&db, "editor").await;
    let mut overrides = std::collections::HashMap::new();
    overrides.insert(OLD.to_string(), -1i64);
    store::set_user_permissions(&db, id, &overrides)
        .await
        .unwrap();

    store::rename_permissions(&db, &permission_renames(&registry()))
        .await
        .unwrap();

    let after = store::load_user_permission_overrides(&db, id)
        .await
        .unwrap();
    assert_eq!(after.get(NEW), Some(&-1), "the deny followed the rename");
    assert!(!after.contains_key(OLD), "and the old key is gone");
}

/// Running it again finds nothing: a boot that repairs nothing stays quiet.
#[tokio::test]
async fn a_second_run_changes_nothing() {
    let (db, _guard) = test_db().await;
    store::create_role(&db, "acme.mine", "Mine", &[OLD.to_string()])
        .await
        .unwrap();
    let renames = permission_renames(&registry());
    assert!(store::rename_permissions(&db, &renames).await.unwrap() >= 1);
    assert_eq!(
        store::rename_permissions(&db, &renames).await.unwrap(),
        0,
        "idempotent"
    );
}

/// A role holding both codes must end with one, not a duplicate.
#[tokio::test]
async fn a_role_holding_both_codes_keeps_one() {
    let (db, _guard) = test_db().await;
    let both = store::create_role(
        &db,
        "acme.both",
        "Both",
        &[OLD.to_string(), NEW.to_string()],
    )
    .await
    .unwrap();
    store::rename_permissions(&db, &permission_renames(&registry()))
        .await
        .unwrap();
    assert_eq!(store::role_permissions(&db, both).await.unwrap(), [NEW]);
}

/// The framework's own roles need no repair; they are rebuilt from the registry.
#[tokio::test]
async fn a_system_role_is_rebuilt_with_the_new_code() {
    let (db, _guard) = test_db().await;
    let perms = registry();
    store::sync_system_roles(&db, &system_roles(&perms))
        .await
        .unwrap();
    let id = store::role_id_by_code(&db, ROLE_EDITOR)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(store::role_permissions(&db, id).await.unwrap(), [NEW]);
}

#[test]
#[should_panic(expected = "both claim the old code")]
fn two_permissions_claiming_one_old_code_abort_the_boot() {
    let _ = permission_renames(&[
        Permission::new("acme.one", t!("One"), t!("Acme")).renamed_from([OLD]),
        Permission::new("acme.two", t!("Two"), t!("Acme")).renamed_from([OLD]),
    ]);
}

#[test]
#[should_panic(expected = "is still registered")]
fn claiming_a_code_that_still_exists_aborts_the_boot() {
    let _ = permission_renames(&[
        Permission::new(OLD, t!("Old"), t!("Acme")),
        Permission::new(NEW, t!("New"), t!("Acme")).renamed_from([OLD]),
    ]);
}
