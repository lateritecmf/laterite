//! A theme as a zip: exported, imported as a new theme, imported over an
//! existing one with and without replace. Imports only the public API.

use laterite_core::{Db, Module};
use laterite_web::theme::{archive, store, Kind, ThemeError, ThemeModule};

async fn test_db() -> (Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

const CONFIG: &str =
    "name: Base\nauthor: Acme\nform:\n  fields:\n    site_name:\n      label: Site name\n      type: text\n";

#[test]
fn a_file_carries_settings_above_the_separator() {
    let body = archive::file_body(
        &serde_json::json!({"url": "/", "layout": "default"}),
        "<h1>x</h1>",
    );
    assert!(body.ends_with("\n==\n<h1>x</h1>"), "{body}");
    let (settings, markup) = archive::parse_body(&body).unwrap();
    assert_eq!(
        settings,
        serde_json::json!({"url": "/", "layout": "default"})
    );
    assert_eq!(markup, "<h1>x</h1>");

    assert_eq!(archive::file_body(&serde_json::json!({}), "plain"), "plain");
    let (settings, markup) = archive::parse_body("no separator\nat all").unwrap();
    assert_eq!(settings, serde_json::json!({}));
    assert_eq!(markup, "no separator\nat all");
    // Text above the separator must be a mapping.
    assert!(archive::parse_body("plain\n==\nx").is_err());
    assert!(archive::parse_body("- a list\n==\nx").is_err());
}

#[tokio::test]
async fn export_then_import_recreates_the_theme() {
    let (db, _g) = test_db().await;
    let base = store::create_theme(&db, "base", CONFIG).await.unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Page,
        "home.htm",
        Some(serde_json::json!({"url": "/"})),
        "<h1>{{ theme.site_name }}</h1>",
        None,
    )
    .await
    .unwrap();
    store::save_template(
        &db,
        base.id,
        Kind::Asset,
        "css/site.css",
        None,
        "body{}",
        None,
    )
    .await
    .unwrap();
    store::save_settings(&db, "base", &serde_json::json!({"site_name": "Acme"}))
        .await
        .unwrap();

    let bytes = archive::export(&db, "base").await.unwrap();
    assert!(bytes.len() > 100);

    let report = archive::import(&db, "copy", &bytes, false, Some(7))
        .await
        .unwrap();
    assert!(report.created);
    assert_eq!(report.imported, 2);
    assert_eq!(report.skipped, 0);
    let copy = store::find_theme(&db, "copy").await.unwrap().unwrap();
    assert_eq!(copy.config.name, "Base");
    assert_eq!(copy.config_source, CONFIG);
    assert_eq!(copy.settings, serde_json::json!({"site_name": "Acme"}));
    let home = store::find_template(&db, copy.id, Kind::Page, "home.htm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(home.settings, serde_json::json!({"url": "/"}));
    assert_eq!(home.markup, "<h1>{{ theme.site_name }}</h1>");
    assert!(
        store::find_template(&db, copy.id, Kind::Asset, "css/site.css")
            .await
            .unwrap()
            .is_some()
    );

    // Into an existing theme: nothing replaced without the flag, everything with it.
    store::save_template(&db, copy.id, Kind::Page, "home.htm", None, "changed", None)
        .await
        .unwrap();
    let report = archive::import(&db, "copy", &bytes, false, None)
        .await
        .unwrap();
    assert_eq!(
        (report.created, report.imported, report.skipped),
        (false, 0, 2)
    );
    assert_eq!(
        store::template(&db, home.id).await.unwrap().unwrap().markup,
        "changed"
    );
    let report = archive::import(&db, "copy", &bytes, true, Some(7))
        .await
        .unwrap();
    assert_eq!(
        (report.created, report.imported, report.skipped),
        (false, 2, 0)
    );
    let home = store::template(&db, home.id).await.unwrap().unwrap();
    assert_eq!(home.markup, "<h1>{{ theme.site_name }}</h1>");
    assert_eq!(store::revisions(&db, home.id).await.unwrap().len(), 2);

    // A locked theme refuses; a new code needs a theme.yaml.
    store::set_locked(&db, "copy", true).await.unwrap();
    assert!(matches!(
        archive::import(&db, "copy", &bytes, true, None).await,
        Err(ThemeError::Locked(_))
    ));
    let mut empty = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    use std::io::Write;
    empty
        .start_file("pages/x.htm", zip::write::SimpleFileOptions::default())
        .unwrap();
    empty.write_all(b"x").unwrap();
    let empty = empty.finish().unwrap().into_inner();
    assert!(matches!(
        archive::import(&db, "fresh", &empty, false, None).await,
        Err(ThemeError::Config(_))
    ));
}
