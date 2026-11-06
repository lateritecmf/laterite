//! A static website built by `Bootstrap` in static mode: the theme folder,
//! a plugin's site files, the config's copies; and a plugin that does not
//! support a static build stops it by name. Imports only the public API.

use laterite_admin::Bootstrap;
use laterite_core::strata::async_trait;
use laterite_core::{CoreResult, Module, ModuleId, SiteFile, SiteFilesCx};
use laterite_web::theme::ThemeModule;
use std::fs;
use std::path::Path;

fn project(root: &Path) {
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/default.toml"),
        "[app]\nname = \"Acme\"\nurl = \"https://acme.example\"\n[theme]\nfolder = \"themes/acme\"\ncopy = [\"static:static\", \"missing:docs\"]\n",
    )
    .unwrap();
    let theme = root.join("themes/acme");
    fs::create_dir_all(theme.join("layouts")).unwrap();
    fs::create_dir_all(theme.join("pages")).unwrap();
    fs::write(theme.join("theme.yaml"), "name: Acme\nauthor: Acme\n").unwrap();
    fs::write(
        theme.join("layouts/default.htm"),
        "<!doctype html><html><head>{{ head }}</head><body>{% block page %}{% endblock %}</body></html>",
    )
    .unwrap();
    fs::write(
        theme.join("pages/home.htm"),
        "---\nurl: /\nlayout: default\ntitle: Acme\n---\n<h1>{{ this.page.title }}</h1>",
    )
    .unwrap();
    fs::write(
        theme.join("pages/about.htm"),
        "---\nurl: /about\nlayout: default\ntitle: About\n---\n<h1>About</h1>",
    )
    .unwrap();
    fs::create_dir_all(root.join("static")).unwrap();
    fs::write(root.join("static/site.css"), "body{}").unwrap();
}

/// A plugin that writes `humans.txt` listing the pages it was told about.
struct Humans;

struct HumansFile(String);

impl SiteFile for HumansFile {
    fn path(&self) -> &str {
        "/humans.txt"
    }
    fn content_type(&self) -> std::borrow::Cow<'static, str> {
        "text/plain".into()
    }
    fn render(&self, cx: &laterite_core::SiteContext) -> String {
        format!("{}\n{}", cx.base_url, self.0)
    }
}

#[async_trait]
impl Module for Humans {
    fn id(&self) -> ModuleId {
        ModuleId::new("acme.humans")
    }
    fn supports_static(&self) -> bool {
        true
    }
    async fn site_files(&self, cx: &SiteFilesCx<'_>) -> CoreResult<Vec<Box<dyn SiteFile>>> {
        Ok(vec![Box::new(HumansFile(cx.pages.join(",")))])
    }
}

/// A plugin that only works with an admin.
struct AdminOnly;

impl Module for AdminOnly {
    fn id(&self) -> ModuleId {
        ModuleId::new("acme.adminonly")
    }
}

#[tokio::test]
async fn a_static_build_writes_pages_plugin_files_and_copies() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let out = tmp.path().join("dist");
    Bootstrap::new(tmp.path().join("config"))
        .module(ThemeModule)
        .modules(vec![Box::new(Humans)])
        .build(&out, None)
        .await
        .unwrap();
    let home = fs::read_to_string(out.join("index.html")).unwrap();
    assert!(home.contains("<h1>Acme</h1>"), "{home}");
    assert!(
        home.contains(r#"<link rel="canonical" href="https://acme.example/">"#),
        "{home}"
    );
    assert!(out.join("about/index.html").is_file());
    let humans = fs::read_to_string(out.join("humans.txt")).unwrap();
    assert_eq!(humans, "https://acme.example\n/,/about");
    assert!(out.join("static/site.css").is_file(), "the config's copy");
    assert!(
        fs::read_to_string(out.join("laterite/htmx.js"))
            .unwrap()
            .contains("htmx"),
        "the request framework"
    );
    assert!(
        fs::read_to_string(out.join("laterite/theme-boot.js"))
            .unwrap()
            .contains("lat-mode"),
        "the colour mode script"
    );
    assert!(
        !out.join("docs").exists(),
        "a missing config copy is skipped"
    );
}

#[tokio::test]
async fn a_plugin_without_static_support_stops_the_build_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let err = Bootstrap::new(tmp.path().join("config"))
        .module(ThemeModule)
        .modules(vec![Box::new(AdminOnly)])
        .build(&tmp.path().join("dist"), None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("acme.adminonly"), "{err:#}");
    assert!(err.to_string().contains("supports_static"), "{err:#}");
}

/// A plugin that stores one file in the media library while the build is
/// seeded, as a module reading its definitions would.
struct Seeder;

#[async_trait]
impl Module for Seeder {
    fn id(&self) -> ModuleId {
        ModuleId::new("acme.seeder")
    }
    fn supports_static(&self) -> bool {
        true
    }
    async fn static_seed(&self, cx: &laterite_core::StaticSeedCx<'_>) -> CoreResult<()> {
        use laterite_media::{ingest::blob_path, LocalDisk, StorageDriver};
        let hash = "ab".repeat(32);
        let disk = LocalDisk::new(cx.root.join("public/media"));
        disk.put(
            &blob_path(&hash),
            laterite_media::stream_of(b"PNGDATA".to_vec()),
        )
        .await
        .map_err(|e| laterite_core::CoreError::Config(e.to_string()))?;
        laterite_media::record::create(
            cx.db,
            laterite_media::NewMedia {
                disk: "public".to_string(),
                hash,
                size: 7,
                content_type: "image/png".to_string(),
                original_name: "logo.png".to_string(),
                in_library: true,
                created_by: None,
            },
        )
        .await
        .map_err(|e| laterite_core::CoreError::Config(e.to_string()))?;
        Ok(())
    }
}

/// The media library's files land in the build where the pages link to
/// them, read from the `[media] folder` the library lives in.
#[tokio::test]
async fn a_static_build_writes_the_media_library() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    fs::write(
        tmp.path().join("config/default.toml"),
        "[app]\nname = \"Acme\"\nurl = \"https://acme.example\"\n[theme]\nfolder = \"themes/acme\"\n[media]\nfolder = \"public/media\"\n",
    )
    .unwrap();
    let out = tmp.path().join("dist");
    Bootstrap::new(tmp.path().join("config"))
        .module(ThemeModule)
        .modules(vec![Box::new(Seeder)])
        .build(&out, None)
        .await
        .unwrap();
    let hash = "ab".repeat(32);
    let served = out.join("storage/media").join(&hash).join("logo.png");
    assert_eq!(
        fs::read(&served).unwrap(),
        b"PNGDATA",
        "{}",
        served.display()
    );
    assert!(
        tmp.path()
            .join("public/media/blobs/ab/ab")
            .join(&hash)
            .is_file(),
        "the blob lives in the project's media folder"
    );
}

/// A theme whose pages carry no URL builds nothing, and the build says so
/// instead of succeeding with an empty output.
#[tokio::test]
async fn a_build_that_writes_no_pages_fails_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let pages = tmp.path().join("themes/acme/pages");
    for entry in fs::read_dir(&pages).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    fs::write(pages.join("home.htm"), "<h1>No front matter, no URL</h1>").unwrap();
    let err = Bootstrap::new(tmp.path().join("config"))
        .module(ThemeModule)
        .build(&tmp.path().join("dist"), None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("wrote no pages"), "{err:#}");
}
