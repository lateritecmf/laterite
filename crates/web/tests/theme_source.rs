//! A theme source keeps the rows in step with where the theme lives: the
//! folder source writes every save back as a file, and reads the folder in
//! on pull. Imports only the public API.

use laterite_core::config::ThemeConfig;
use laterite_core::{CatalogStore, Module};
use laterite_web::theme::source::{Folder, SiteProject, SourceCx, ThemeSource};
use laterite_web::theme::{store, Kind, ThemeModule, Themes};
use std::fs;
use std::path::Path;
use std::sync::Arc;

async fn test_db() -> (laterite_core::Db, laterite_core::testing::TestGuard) {
    let mut migrations = laterite_admin::builtin_migrations();
    migrations.push(ThemeModule.migrations());
    laterite_core::testing::connect_test(&migrations).await
}

fn theme_folder(root: &Path) {
    let dir = root.join("themes/acme");
    fs::create_dir_all(dir.join("layouts")).unwrap();
    fs::create_dir_all(dir.join("pages")).unwrap();
    fs::write(dir.join("theme.yaml"), "name: Acme\nauthor: Acme\n").unwrap();
    fs::write(
        dir.join("layouts/default.htm"),
        "<html>{% block page %}{% endblock %}</html>",
    )
    .unwrap();
    fs::write(
        dir.join("pages/home.htm"),
        "---\nurl: /\nlayout: default\n---\n<h1>Home</h1>",
    )
    .unwrap();
}

fn config() -> ThemeConfig {
    ThemeConfig {
        folder: Some("themes/acme".to_string()),
        source: "folder".to_string(),
        ..ThemeConfig::default()
    }
}

#[tokio::test]
async fn the_folder_source_writes_every_save_back_and_reads_the_folder_in() {
    let (db, _guard) = test_db().await;
    let tmp = tempfile::tempdir().unwrap();
    theme_folder(tmp.path());
    let project = SiteProject {
        root: tmp.path().to_path_buf(),
        theme: config(),
    };
    let theme_config = config();
    let cx = SourceCx {
        db: &db,
        root: tmp.path(),
        config: &theme_config,
    };
    Folder.pull(&cx).await.unwrap();
    let theme = store::find_theme(&db, "acme")
        .await
        .unwrap()
        .expect("pulled");

    let cache = laterite_core::Cache::local();
    let themes = Themes::new(db.clone(), cache).with_source(Arc::new(Folder), project);
    themes
        .save_template(
            theme.id,
            Kind::Page,
            "about.htm",
            Some(serde_json::json!({"url": "/about", "layout": "default"})),
            "<h1>About</h1>",
            None,
        )
        .await
        .unwrap();
    let about = fs::read_to_string(tmp.path().join("themes/acme/pages/about.htm")).unwrap();
    assert!(
        about.starts_with("---\nlayout: default\nurl: /about\n---\n")
            || about.contains("url: /about"),
        "{about}"
    );
    assert!(about.ends_with("<h1>About</h1>"), "{about}");

    themes
        .update_theme_config("acme", "name: Acme Two\nauthor: Acme\n")
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(tmp.path().join("themes/acme/theme.yaml")).unwrap(),
        "name: Acme Two\nauthor: Acme\n"
    );
    themes
        .save_settings("acme", &serde_json::json!({"accent": "red"}))
        .await
        .unwrap();
    let settings = fs::read_to_string(tmp.path().join("themes/acme/settings.json")).unwrap();
    assert!(settings.contains("\"accent\": \"red\""), "{settings}");

    let saved = store::find_template(&db, theme.id, Kind::Page, "about.htm")
        .await
        .unwrap()
        .unwrap();
    themes.delete_template(saved.id).await.unwrap();
    assert!(!tmp.path().join("themes/acme/pages/about.htm").exists());

    // A change to another theme stays in the rows: the folder keeps its own.
    let other = themes
        .create_theme("other", "name: Other\nauthor: Acme\n")
        .await
        .unwrap();
    themes
        .save_template(other.id, Kind::Page, "x.htm", None, "<p>x</p>", None)
        .await
        .unwrap();
    assert!(!tmp.path().join("themes/other").exists());

    // The folder wins on the next pull: an edit on disk replaces the rows.
    fs::write(
        tmp.path().join("themes/acme/pages/home.htm"),
        "---\nurl: /\nlayout: default\n---\n<h1>Edited on disk</h1>",
    )
    .unwrap();
    Folder.pull(&cx).await.unwrap();
    let theme = store::find_theme(&db, "acme").await.unwrap().unwrap();
    let home = store::find_template(&db, theme.id, Kind::Page, "home.htm")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(home.markup, "<h1>Edited on disk</h1>");
    assert!(themes.publish().await.unwrap().note.contains("commit"));
}

#[tokio::test]
async fn an_unknown_source_is_refused_by_name() {
    let registry = laterite_core::Registry::new();
    let config = ThemeConfig {
        source: "ftp".to_string(),
        ..ThemeConfig::default()
    };
    let err = laterite_web::theme::source::resolve(&registry, &config)
        .err()
        .expect("refused");
    assert!(err.to_string().contains("\"ftp\""), "{err}");
}

#[cfg(feature = "theme-git")]
#[tokio::test]
async fn the_git_source_commits_every_save() {
    use laterite_web::theme::source::Git;
    let (db, _guard) = test_db().await;
    let tmp = tempfile::tempdir().unwrap();
    theme_folder(tmp.path());
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@acme.example"]);
    git(&["config", "user.name", "Test"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "Initial"]);

    let config = ThemeConfig {
        folder: Some("themes/acme".to_string()),
        source: "git".to_string(),
        ..ThemeConfig::default()
    };
    let project = SiteProject {
        root: tmp.path().to_path_buf(),
        theme: config.clone(),
    };
    let cx = SourceCx {
        db: &db,
        root: tmp.path(),
        config: &config,
    };
    Git.pull(&cx).await.unwrap();
    let theme = store::find_theme(&db, "acme").await.unwrap().unwrap();
    let themes =
        Themes::new(db.clone(), laterite_core::Cache::local()).with_source(Arc::new(Git), project);
    themes
        .save_template(
            theme.id,
            Kind::Page,
            "about.htm",
            None,
            "<h1>About</h1>",
            None,
        )
        .await
        .unwrap();
    let log = git(&["log", "--oneline"]);
    assert!(log.contains("Saved pages/about.htm"), "{log}");
    assert!(
        git(&["status", "--porcelain"]).trim().is_empty(),
        "clean after the commit"
    );
    // No remote: publishing says so instead of pretending.
    let err = themes.publish().await.unwrap_err();
    assert!(err.to_string().contains("git push"), "{err}");
}

#[allow(dead_code)]
fn _catalogs(_: Arc<CatalogStore>) {}
