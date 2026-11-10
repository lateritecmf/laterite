//! The crates whose types cross the plugin boundary are reached through the
//! framework, at the framework's versions.

use lateritecmf::sea_query::{self, Iden, Query, SqliteQueryBuilder};

#[derive(Iden)]
enum AcmePosts {
    Table,
    Id,
}

#[test]
fn the_reexported_crates_are_the_frameworks_own() {
    // The pool a plugin is handed is the re-exported crate's type.
    let pool: fn(&lateritecmf::core::Db) -> &lateritecmf::sqlx::AnyPool = |db| &db.pool;
    let _ = pool;

    let sql = Query::select()
        .column(AcmePosts::Id)
        .from(AcmePosts::Table)
        .to_string(SqliteQueryBuilder);
    assert_eq!(sql, r#"SELECT "id" FROM "acme_posts""#);

    let value: lateritecmf::serde_json::Value = lateritecmf::serde_json::json!({ "n": 1 });
    assert_eq!(value["n"], 1);
    let env = lateritecmf::minijinja::Environment::new();
    let out = env
        .render_str("{{ n }}", lateritecmf::minijinja::context! { n => 1 })
        .unwrap();
    assert_eq!(out, "1");

    let _router: lateritecmf::axum::Router = lateritecmf::axum::Router::new();
}

#[test]
fn laterite_core_carries_the_same_crates() {
    let a: lateritecmf::serde_json::Value = lateritecmf::core::serde_json::Value::Null;
    assert!(a.is_null());
}
