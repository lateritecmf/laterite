//! `lat make:migration`: scaffolds a one-file migration.
//!
//! Migrations live one-per-file under a crate's `src/migrations/` directory,
//! listed in `mod.rs` by the `migration_set!` macro in apply order. This command
//! finds the next sequence number, writes a blueprint file named `m<NNNN>_<slug>`
//! from the description, and appends it to the manifest, so the structure stays
//! consistent whether the framework, an application, or a plugin owns it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;

#[derive(Args)]
pub struct MakeMigrationArgs {
    /// A short description, for example `create_events` or "add user avatar".
    /// It becomes the file name and the migration's stable `name`.
    description: String,
    /// The crate directory to scaffold in. Defaults to the current directory,
    /// which must hold a `src/migrations/mod.rs`.
    #[arg(long, default_value = ".")]
    path: PathBuf,
}

pub fn run(args: MakeMigrationArgs) -> Result<()> {
    let migrations_dir = args.path.join("src").join("migrations");
    let manifest = migrations_dir.join("mod.rs");
    if !manifest.is_file() {
        bail!(
            "no migration manifest at {}.\nRun this from a Laterite application or \
             plugin crate (one with a src/migrations/mod.rs), or pass --path.",
            manifest.display()
        );
    }

    let slug = slug(&args.description);
    if slug.is_empty() {
        bail!("the description must contain at least one letter or digit");
    }

    let next = next_sequence(&migrations_dir)?;
    let name = format!("{next:04}_{slug}");
    let module = format!("m{name}");
    let file = migrations_dir.join(format!("{module}.rs"));
    if file.exists() {
        bail!("{} already exists", file.display());
    }

    fs::write(&file, blueprint(&name, &args.description))
        .with_context(|| format!("writing {}", file.display()))?;
    append_to_manifest(&manifest, &module)?;

    println!("Created migration {}", file.display());
    println!("Listed {module} in {}", manifest.display());
    Ok(())
}

/// Slugifies a description into snake_case: lower-case alphanumerics, every other
/// run collapsed to a single `_`, with no leading or trailing underscore.
fn slug(input: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending && !out.is_empty() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            pending = false;
        } else {
            pending = true;
        }
    }
    out
}

/// The next four-digit sequence number: one past the highest `m<NNNN>_` file
/// already in the directory, or `1` when there are none.
fn next_sequence(dir: &Path) -> Result<u32> {
    let mut max = 0u32;
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix('m') {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if digits.len() == 4 {
                if let Ok(n) = digits.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }
    Ok(max + 1)
}

/// Inserts the new module into the `migration_set!` block, before its closing
/// brace, preserving the rest of the manifest verbatim.
fn append_to_manifest(manifest: &Path, module: &str) -> Result<()> {
    let src =
        fs::read_to_string(manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    // The `migration_set!` block closes at the first bare `}` line after its
    // opening; the module impl that follows it in an application has its own.
    let open = lines
        .iter()
        .position(|l| l.contains("migration_set!"))
        .context("could not find the migration_set! block to extend")?;
    let close = lines[open..]
        .iter()
        .position(|l| l.trim() == "}")
        .map(|i| open + i)
        .context("could not find the end of the migration_set! block")?;
    lines.insert(close, format!("    {module},"));
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(manifest, out).with_context(|| format!("writing {}", manifest.display()))
}

/// The scaffold written to a new migration file.
fn blueprint(name: &str, description: &str) -> String {
    let doc = describe(description);
    format!(
        r#"//! {doc}

use laterite_core::strata::*;

/// The `{name}` migration.
pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {{
    fn name(&self) -> &str {{
        "{name}"
    }}

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {{
        // Build the schema change here. For example:
        //
        //     s.exec(
        //         Table::create()
        //             .table(/* your table */)
        //             .if_not_exists()
        //             .col(
        //                 ColumnDef::new(/* id column */)
        //                     .big_integer()
        //                     .not_null()
        //                     .auto_increment()
        //                     .primary_key(),
        //             )
        //             .to_owned(),
        //     )
        //     .await
        let _ = s;
        todo!("write the up migration")
    }}

    // Reversing is opt-in: with no `down`, this migration is irreversible. Add
    // one to make `lat migrate rollback` work, for example:
    //
    //     async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {{
    //         s.exec(Table::drop().table(/* your table */).to_owned()).await
    //     }}
}}
"#
    )
}

/// Turns a raw description into a doc-comment sentence: underscores to spaces,
/// first letter capitalised, a trailing period ensured.
fn describe(description: &str) -> String {
    let spaced = description.replace('_', " ");
    let trimmed = spaced.trim();
    let mut chars = trimmed.chars();
    let mut out = match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_collapses_separators() {
        assert_eq!(slug("create_events"), "create_events");
        assert_eq!(slug("add user avatar"), "add_user_avatar");
        assert_eq!(slug("  Add  User--Avatar!! "), "add_user_avatar");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn describe_reads_as_a_sentence() {
        assert_eq!(describe("create_events"), "Create events.");
        assert_eq!(describe("add user avatar"), "Add user avatar.");
    }

    #[test]
    fn next_sequence_follows_the_highest_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(next_sequence(dir).unwrap(), 1);
        fs::write(dir.join("mod.rs"), "").unwrap();
        fs::write(dir.join("m0001_create_events.rs"), "").unwrap();
        fs::write(dir.join("m0002_add_flags.rs"), "").unwrap();
        assert_eq!(next_sequence(dir).unwrap(), 3);
    }

    #[test]
    fn make_scaffolds_and_lists_the_migration() {
        let tmp = tempfile::tempdir().unwrap();
        let crate_dir = tmp.path();
        let migrations = crate_dir.join("src").join("migrations");
        fs::create_dir_all(&migrations).unwrap();
        fs::write(
            migrations.join("mod.rs"),
            "//! Manifest.\n\nlaterite_core::migration_set! {\n    module_id: \"acme\",\n}\n",
        )
        .unwrap();

        run(MakeMigrationArgs {
            description: "create events".to_string(),
            path: crate_dir.to_path_buf(),
        })
        .unwrap();

        let file = migrations.join("m0001_create_events.rs");
        assert!(file.exists());
        let body = fs::read_to_string(&file).unwrap();
        assert!(body.contains("\"0001_create_events\""));
        assert!(body.contains("pub struct Migration;"));

        let manifest = fs::read_to_string(migrations.join("mod.rs")).unwrap();
        assert!(manifest.contains("    m0001_create_events,"));
        // The entry sits inside the block, above its closing brace.
        let entry = manifest.find("m0001_create_events").unwrap();
        let close = manifest.rfind('}').unwrap();
        assert!(entry < close);

        // A second run picks up the next number.
        run(MakeMigrationArgs {
            description: "add flags".to_string(),
            path: crate_dir.to_path_buf(),
        })
        .unwrap();
        assert!(migrations.join("m0002_add_flags.rs").exists());
    }

    /// `--source` keys the resource by a slug, adds the body for markdown,
    #[test]
    fn make_model_writes_a_typed_struct_and_wires_the_module() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.rs"), "//! A plugin.\n").unwrap();
        run_model(MakeModelArgs {
            name: "article".to_string(),
            path: tmp.path().to_path_buf(),
            field: vec![
                "title:text".into(),
                "views:integer".into(),
                "published_at:datetime".into(),
            ],
            table: None,
        })
        .unwrap();
        let model = fs::read_to_string(src.join("models/article.rs")).unwrap();
        assert!(model.contains("#[model(table = \"articles\")]"), "{model}");
        assert!(model.contains("pub struct Article {"), "{model}");
        assert!(model.contains("    pub views: i64,"), "{model}");
        assert!(
            model.contains("    pub published_at: Option<DateTime<Utc>>,"),
            "{model}"
        );
        assert!(
            model.contains("use laterite_core::model::{DateTime, Utc};"),
            "{model}"
        );
        assert!(fs::read_to_string(src.join("models/mod.rs"))
            .unwrap()
            .contains("pub mod article;"));
        assert!(fs::read_to_string(src.join("lib.rs"))
            .unwrap()
            .contains("pub mod models;"));
        // Running it again refuses rather than overwriting.
        assert!(run_model(MakeModelArgs {
            name: "article".to_string(),
            path: tmp.path().to_path_buf(),
            field: Vec::new(),
            table: None,
        })
        .is_err());
    }

    /// and declares the source on the resource.
    #[test]
    fn make_resource_with_a_source_scaffolds_a_file_backed_resource() {
        let tmp = tempfile::tempdir().unwrap();
        let crate_dir = tmp.path();
        let src = crate_dir.join("src");
        fs::create_dir_all(src.join("migrations")).unwrap();
        fs::write(src.join("lib.rs"), "//! A plugin.\n").unwrap();
        fs::write(
            src.join("migrations").join("mod.rs"),
            "//! Manifest.\n\nlaterite_core::migration_set! {\n    module_id: \"acme.blog\",\n}\n",
        )
        .unwrap();

        run_resource(MakeResourceArgs {
            name: "article".to_string(),
            path: crate_dir.to_path_buf(),
            field: vec!["title:text".to_string(), "published:boolean".to_string()],
            source: Some("content/articles".to_string()),
            format: "markdown".to_string(),
            git: true,
        })
        .unwrap();

        let admin = fs::read_to_string(src.join("admin").join("article.rs")).unwrap();
        assert!(admin.contains("use laterite_core::source::{Format, Source};"));
        assert!(admin.contains("Source::git(\"content/articles\", Format::Markdown)"));
        assert!(admin.contains(".body(\"body\")"));
        assert!(admin.contains(".slug_from(\"title\")"));
        assert!(admin.contains("FormField::text(\"slug\", \"Slug\").unique(),"));
        assert!(admin.contains("FormField::textarea(\"body\", \"Body\"),"));
        assert!(admin.contains("FormField::switch(\"published\", \"Published\"),"));

        // A plugin scaffolded with `--admin` has its first resource registered.
        let plugin = tempfile::tempdir().unwrap();
        run_plugin(MakePluginArgs {
            id: "acme.site".to_string(),
            path: plugin.path().to_path_buf(),
            admin: true,
            r#static: true,
        })
        .unwrap();
        let plugin_dir = plugin.path().join("site-plugin");
        let cargo = fs::read_to_string(plugin_dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("static = true"), "{cargo}");
        let lib = fs::read_to_string(plugin_dir.join("src/lib.rs")).unwrap();
        assert!(lib.contains("fn supports_static(&self) -> bool"), "{lib}");
        assert!(
            cargo.contains(&format!("laterite-core = \"{}\"", framework_requirement())),
            "{cargo}"
        );
        run_resource(MakeResourceArgs {
            name: "post".to_string(),
            path: plugin_dir.clone(),
            field: vec!["title:text".to_string()],
            source: Some("content/posts".to_string()),
            format: "markdown".to_string(),
            git: false,
        })
        .unwrap();
        let lib = fs::read_to_string(plugin_dir.join("src/lib.rs")).unwrap();
        assert!(
            lib.contains("registry.add_resource(admin::post::resource());"),
            "{lib}"
        );
        assert!(lib.contains("registry.add_permission(admin::post::permission());"));
        assert!(!lib.contains("let _ = registry;"));
        assert!(lib.contains("mod site;"), "{lib}");
        assert!(
            lib.contains("        site::post::register(registry);"),
            "{lib}"
        );
        let site = fs::read_to_string(plugin_dir.join("src/site/post.rs")).unwrap();
        assert!(site.contains("\"acme.site.posts\""), "{site}");
        assert!(site.contains("\"/posts/:slug\""), "{site}");
        assert!(
            site.contains("const BODY: Option<&str> = Some(\"body\");"),
            "{site}"
        );
        let cargo = fs::read_to_string(plugin_dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("laterite-web = "), "{cargo}");
        assert!(cargo.contains("serde_json = \"1\""), "{cargo}");

        let migration =
            fs::read_to_string(src.join("migrations").join("m0001_create_articles.rs")).unwrap();
        assert!(migration.contains("Articles::Slug"));
        assert!(migration.contains("Articles::Body"));

        // Without a source nothing of it appears.
        let plain = resource_admin(
            "acme.blog",
            "note",
            "notes",
            &[Field {
                name: "title".to_string(),
                kind: FieldKind::Text,
            }],
            None,
        );
        assert!(!plain.contains(".source("));
    }

    /// An application's manifest carries its module impl after the set; the
    /// entry goes into the set, never into the impl's braces.
    #[test]
    fn make_lists_inside_the_set_not_the_module_impl() {
        let tmp = tempfile::tempdir().unwrap();
        let migrations = tmp.path().join("src").join("migrations");
        fs::create_dir_all(&migrations).unwrap();
        fs::write(
            migrations.join("mod.rs"),
            "use lateritecmf::prelude::*;\n\nmigration_set! {\n    module_id: \"acme\",\n}\n\n\
             pub struct AppModule;\n\nimpl Module for AppModule {\n    fn id(&self) -> ModuleId {\n        \
             ModuleId::new(MODULE_ID)\n    }\n}\n",
        )
        .unwrap();

        run(MakeMigrationArgs {
            description: "create events".to_string(),
            path: tmp.path().to_path_buf(),
        })
        .unwrap();

        let manifest = fs::read_to_string(migrations.join("mod.rs")).unwrap();
        let entry = manifest.find("    m0001_create_events,").unwrap();
        let set_close = manifest.find("\n}\n").unwrap();
        assert!(entry < set_close, "{manifest}");
        assert!(manifest.ends_with("ModuleId::new(MODULE_ID)\n    }\n}\n"));
    }

    #[test]
    fn make_without_a_manifest_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run(MakeMigrationArgs {
            description: "create events".to_string(),
            path: tmp.path().to_path_buf(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("no migration manifest"));
    }
}

/// `lat make:plugin`: scaffold a plugin crate in the expected layout.
#[derive(Args)]
pub struct MakePluginArgs {
    /// The module id the plugin registers, as `vendor.name` (e.g. `acme.blog`).
    /// The crate is named `vendor-name` and the folder after it.
    pub(crate) id: String,
    /// Where to create it. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub(crate) path: PathBuf,
    /// Scaffold the admin half (descriptors, screens, settings).
    #[arg(long)]
    pub(crate) admin: bool,
    /// Declare that the plugin builds into a static website (`static = true`
    /// in its marker).
    #[arg(long)]
    pub(crate) r#static: bool,
}

/// The framework version requirement a scaffold writes: this tool's own
/// major and minor, so a generated crate resolves to the framework it was
/// made with.
pub(crate) fn framework_requirement() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut parts = version.split('.');
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => format!("{major}.{minor}"),
        _ => version.to_string(),
    }
}

pub fn run_plugin(args: MakePluginArgs) -> Result<()> {
    let (vendor, name) = args
        .id
        .split_once('.')
        .filter(|(v, n)| !v.is_empty() && !n.is_empty())
        .with_context(|| {
            format!(
                "`{}` is not a module id. It is `vendor.name`, lowercase, e.g. acme.blog",
                args.id
            )
        })?;
    if !ident_ok(vendor) || !ident_ok(name) {
        bail!("a module id is lowercase letters, digits and underscores, as `vendor.name`");
    }

    let crate_name = format!("{vendor}-{name}");
    let root = args.path.join(format!("{name}-plugin"));
    if root.exists() {
        bail!("{} already exists", root.display());
    }

    // Only what every plugin has. A plugin with no tables (one that ships
    // settings, or routes) must not begin by deleting a schema and a store it
    // never wanted; `make:resource` creates those the moment there is something
    // to put in them.
    fs::create_dir_all(root.join("src"))?;
    write(
        &root.join("Cargo.toml"),
        &plugin_cargo(&crate_name, &args.id, args.r#static),
    )?;
    write(
        &root.join("src/lib.rs"),
        &plugin_lib(&args.id, args.admin, args.r#static),
    )?;
    write(&root.join(".gitignore"), "/target\n")?;
    if args.admin {
        fs::create_dir_all(root.join("src/admin"))?;
        write(&root.join("src/admin/mod.rs"), PLUGIN_ADMIN)?;
    }

    println!("Created {}", root.display());
    println!("  {crate_name}, registering `{}`", args.id);
    println!("\nAdd it to an application with:");
    println!("  lat plugin add {}", root.display());
    Ok(())
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn write(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

fn plugin_cargo(crate_name: &str, id: &str, static_site: bool) -> String {
    let req = framework_requirement();
    let static_line = if static_site { "static = true\n" } else { "" };
    format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
description = "A Laterite plugin"
edition = "2021"
license = "MIT OR Apache-2.0"

# Marks this crate as a Laterite plugin and names the module it registers, so a
# tool can tell what it is without building it. Which Laterite it supports is
# stated by the dependency requirement below, never repeated here.
[package.metadata.laterite]
plugin = "{id}"
{static_line}
[dependencies]
# Version-only, so this manifest does not depend on where the crate sits. An
# application building against a local framework checkout resolves these through
# its own [patch.crates-io].
laterite-core = "{req}"
laterite-admin = "{req}"
# The schema identifier enums use sea-query's `Iden` derive, whose generated code
# names the crate directly.
sea-query = {{ version = "0.32", features = ["derive"] }}
sqlx = {{ version = "0.8", default-features = false, features = [
    "runtime-tokio",
    "tls-rustls",
    "any",
] }}
chrono = {{ version = "0.4", default-features = false, features = ["clock"] }}

[dev-dependencies]
laterite-core = {{ version = "{req}", features = ["sqlite", "testing"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#
    )
}

fn plugin_lib(id: &str, admin: bool, static_site: bool) -> String {
    let admin_mod = if admin { "mod admin;\n\n" } else { "" };
    let register = if admin {
        "\n    fn register(&self, registry: &mut Registry) {\n        \
         use laterite_admin::AdminRegistry;\n        let _ = registry;\n        \
         // registry.add_permission(admin::thing::permission());\n        \
         // registry.add_resource(admin::thing::resource());\n    }\n"
    } else {
        ""
    };
    let registry_import = if admin { ", Registry" } else { "" };
    let static_impl = if static_site {
        "\n    /// Builds into a static website: its tables are read at build time\n    \
         /// and its records live in the site's files.\n    \
         fn supports_static(&self) -> bool {\n        true\n    }\n"
    } else {
        ""
    };
    format!(
        r#"//! A Laterite plugin.

{admin_mod}use laterite_core::{{MigrationSet, Module, ModuleId{registry_import}}};

/// Every plugin exposes `module()` at its crate root, which is what the
/// generated plugin manifest calls. One behind a feature, or in a submodule,
/// compiles cleanly and registers nothing.
pub fn module() -> Box<dyn Module> {{
    Box::new(Plugin)
}}

pub struct Plugin;

impl Module for Plugin {{
    fn id(&self) -> ModuleId {{
        ModuleId::new("{id}")
    }}

    /// None yet. `lat make:resource` creates `src/migrations/` with the first
    /// one; point this at `migrations::migrations()` when it does.
    fn migrations(&self) -> MigrationSet {{
        MigrationSet::new("{id}", Vec::new())
    }}
{static_impl}{register}}}
"#
    )
}

fn plugin_migrations(id: &str) -> String {
    format!(
        r#"//! This plugin's schema, as portable one-file migrations.
//!
//! Each migration is one file, listed below in apply order. Append new entries
//! at the end; never reorder or rename a shipped one. Add one with
//! `lat make:migration <description>`.

laterite_core::migration_set! {{
    module_id: "{id}",
}}
"#
    )
}

const PLUGIN_ADMIN: &str = r#"//! This plugin's admin surface: descriptors, screens and settings.
//!
//! Screens are data. Reach for a `Resource` first and a `Screen` only for what a
//! list and a form cannot express.

#[allow(unused_imports)]
use laterite_admin::{list::ListConfig, Permission, Resource};
"#;

/// `lat make:resource`: the whole vertical slice for one entity.
///
/// The command that turns an empty plugin into a working admin screen: a
/// migration, the table identifiers, a store, and the list and form descriptors
/// that render it. Emitting them separately would leave an author wiring four
/// files together by hand, which is where a layout drifts.
#[derive(Args)]
pub struct MakeResourceArgs {
    /// The entity, singular and snake_case (e.g. `article`). The table is its
    /// plural, and the screen mounts at `/articles`.
    pub(crate) name: String,
    /// The crate to scaffold in. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub(crate) path: PathBuf,
    /// Columns beyond the id and timestamps, as `name:type`. Types: text,
    /// integer, boolean, datetime. Repeat the flag or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub(crate) field: Vec<String>,
    /// Keep the records as files in this folder of the project
    /// (`content/articles`): a `slug` column names each file.
    #[arg(long)]
    pub(crate) source: Option<String>,
    /// The file format with `--source`: yaml, json or markdown (front matter
    /// and a `body` column).
    #[arg(long, default_value = "yaml")]
    pub(crate) format: String,
    /// With `--source`, commit every save.
    #[arg(long)]
    pub(crate) git: bool,
}

/// What `--source` adds to the scaffold.
struct SourceShape {
    path: String,
    format: String,
    git: bool,
}

impl SourceShape {
    fn parse(args: &MakeResourceArgs) -> Result<Option<Self>> {
        let Some(path) = &args.source else {
            return Ok(None);
        };
        let path = path.trim().trim_matches('/').to_string();
        if path.is_empty() || path.starts_with("..") || path.contains("/../") {
            bail!("--source is a folder inside the project, like content/articles");
        }
        let format = match args.format.trim() {
            "yaml" | "yml" => "yaml",
            "json" => "json",
            "markdown" | "md" => "markdown",
            other => bail!("unknown format `{other}`. Use yaml, json or markdown"),
        };
        Ok(Some(Self {
            path,
            format: format.to_string(),
            git: args.git,
        }))
    }

    fn markdown(&self) -> bool {
        self.format == "markdown"
    }
}

/// A column an author asked for.
struct Field {
    name: String,
    kind: FieldKind,
}

#[derive(Clone, Copy, PartialEq)]
enum FieldKind {
    Text,
    Integer,
    Boolean,
    DateTime,
}

impl FieldKind {
    fn parse(raw: &str) -> Result<Self> {
        Ok(match raw {
            "text" | "string" => Self::Text,
            "integer" | "int" => Self::Integer,
            "boolean" | "bool" => Self::Boolean,
            "datetime" | "date" => Self::DateTime,
            other => bail!("unknown field type `{other}`. Use text, integer, boolean or datetime"),
        })
    }

    /// The schema-builder call for this column.
    fn column(&self, ident: &str) -> String {
        match self {
            Self::Text => format!("ColumnDef::new({ident}).text()"),
            Self::Integer => format!("ColumnDef::new({ident}).big_integer()"),
            Self::Boolean => format!("bool_col({ident}).not_null().default(0)"),
            Self::DateTime => format!("ColumnDef::new({ident}).text()"),
        }
    }

    /// The list column type, where it is not plain text.
    fn column_type(&self) -> &'static str {
        match self {
            Self::Boolean => ".yes_no()",
            Self::DateTime => ".datetime()",
            _ => "",
        }
    }

    /// The form field constructor.
    fn form_field(&self) -> &'static str {
        match self {
            Self::Boolean => "switch",
            Self::DateTime => "date",
            _ => "text",
        }
    }
}

/// `lat make:model`: a typed struct for a table, read and written through
/// `laterite_core::model`.
#[derive(Args)]
pub struct MakeModelArgs {
    /// The entity, singular and snake_case (e.g. `article`). The table is its
    /// plural, the struct its PascalCase name.
    pub(crate) name: String,
    /// The crate to scaffold in. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub(crate) path: PathBuf,
    /// Columns beyond the id, as `name:type`. Types: text, integer, boolean,
    /// datetime. Repeat the flag or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub(crate) field: Vec<String>,
    /// The table, when it is not the entity's plural.
    #[arg(long)]
    pub(crate) table: Option<String>,
}

impl FieldKind {
    /// The Rust type a model field of this kind holds.
    fn rust_type(&self) -> &'static str {
        match self {
            Self::Text => "String",
            Self::Integer => "i64",
            Self::Boolean => "bool",
            Self::DateTime => "Option<DateTime<Utc>>",
        }
    }
}

fn parse_fields(raw: &[String]) -> Result<Vec<Field>> {
    raw.iter()
        .filter(|f| !f.trim().is_empty())
        .map(|raw| {
            let (name, kind) = raw.split_once(':').unwrap_or((raw.as_str(), "text"));
            let name = slug(name);
            if name.is_empty() {
                bail!("a field needs a name, as `name:type`");
            }
            Ok(Field {
                kind: FieldKind::parse(kind.trim())?,
                name,
            })
        })
        .collect()
}

pub fn run_model(args: MakeModelArgs) -> Result<()> {
    let entity = slug(&args.name);
    if entity.is_empty() {
        bail!("the name must contain at least one letter or digit");
    }
    let src = args.path.join("src");
    let lib = src.join("lib.rs");
    if !lib.is_file() {
        bail!(
            "no src/lib.rs at {}.\nRun this from a plugin or application crate, or pass --path.",
            args.path.display()
        );
    }
    let fields = parse_fields(&args.field)?;
    if fields.iter().any(|f| f.name == "id") {
        bail!("the id column comes with every model; leave it out of --field");
    }
    let table = args
        .table
        .as_deref()
        .map(slug)
        .unwrap_or_else(|| plural(&entity));
    let models = src.join("models");
    let file = models.join(format!("{entity}.rs"));
    if file.exists() {
        bail!("{} already exists", file.display());
    }
    fs::create_dir_all(&models).with_context(|| format!("creating {}", models.display()))?;
    write(&file, &model_source(&entity, &table, &fields))?;

    let index = models.join("mod.rs");
    let mut listing = fs::read_to_string(&index)
        .unwrap_or_else(|_| "//! Typed models over this crate's tables.\n\n".to_string());
    let line = format!("pub mod {entity};\n");
    if !listing.contains(&line) {
        listing.push_str(&line);
        write(&index, &listing)?;
    }
    let lib_text =
        fs::read_to_string(&lib).with_context(|| format!("reading {}", lib.display()))?;
    if !lib_text.contains("mod models;") {
        write(&lib, &format!("{}\npub mod models;\n", lib_text.trim_end()))?;
    }
    println!("Wrote {}", file.display());
    Ok(())
}

fn model_source(entity: &str, table: &str, fields: &[Field]) -> String {
    let type_name = pascal(entity);
    let dated = fields.iter().any(|f| f.kind == FieldKind::DateTime);
    let columns: String = fields
        .iter()
        .map(|f| format!("    pub {}: {},\n", f.name, f.kind.rust_type()))
        .collect();
    let imports = if dated {
        "use laterite_core::model::{DateTime, Utc};\n\n"
    } else {
        ""
    };
    format!(
        "//! A row of `{table}`, read and written through `laterite_core::model`:\n\
         //! `model::find::<{type_name}>(&db, id)`, `model::insert(&db, &row)`.\n\n\
         {imports}\
         #[derive(Debug, Clone, laterite_core::Model)]\n\
         #[model(table = \"{table}\")]\n\
         pub struct {type_name} {{\n    pub id: i64,\n{columns}}}\n"
    )
}

pub fn run_resource(args: MakeResourceArgs) -> Result<()> {
    let entity = slug(&args.name);
    if entity.is_empty() {
        bail!("the name must contain at least one letter or digit");
    }
    let src = args.path.join("src");
    if !src.join("lib.rs").is_file() {
        bail!(
            "no src/lib.rs at {}.\nRun this from a plugin or application crate, or pass --path.",
            args.path.display()
        );
    }

    let source = SourceShape::parse(&args)?;
    let mut fields: Vec<Field> = parse_fields(&args.field)?;
    // A sourced resource is keyed by its slug, first on the form; a markdown
    // one holds its body under the front matter.
    if let Some(shape) = &source {
        if !fields.iter().any(|f| f.name == "slug") {
            fields.insert(
                0,
                Field {
                    name: "slug".to_string(),
                    kind: FieldKind::Text,
                },
            );
        }
        if shape.markdown() && !fields.iter().any(|f| f.name == "body") {
            fields.push(Field {
                name: "body".to_string(),
                kind: FieldKind::Text,
            });
        }
    }

    // The permission is namespaced by the module this crate registers, so two
    // plugins with an `article` resource do not fight over `article.manage`.
    // Read from the plugin marker, which is what it is for.
    let module_id = crate::plugin::declared_module(&args.path).unwrap_or_default();

    let table = plural(&entity);
    // The table's identifier enum is named for the table, the row struct for the
    // entity, so `Articles::Title` is a column and `Article` is one row. Naming
    // both after the entity makes `Article::Id` ambiguous.
    let type_name = pascal(&table);

    // The migration, through the same path `make:migration` takes, so both
    // commands agree on numbering and on listing it in the manifest.
    let migrations_dir = src.join("migrations");
    if !migrations_dir.join("mod.rs").is_file() && !module_id.is_empty() {
        fs::create_dir_all(&migrations_dir)?;
        write(
            &migrations_dir.join("mod.rs"),
            &plugin_migrations(&module_id),
        )?;
    }
    let migration = if migrations_dir.join("mod.rs").is_file() {
        let next = next_sequence(&migrations_dir)?;
        let name = format!("{next:04}_create_{table}");
        let module = format!("m{name}");
        let file = migrations_dir.join(format!("{module}.rs"));
        if file.exists() {
            bail!("{} already exists", file.display());
        }
        fs::write(
            &file,
            resource_migration(&name, &table, &type_name, &fields),
        )?;
        append_to_manifest(&migrations_dir.join("mod.rs"), &module)?;
        Some(file)
    } else {
        None
    };

    let schema = src.join("schema.rs");
    append_or_create(
        &schema,
        "//! Table and column identifiers for this plugin's schema.\n\nuse laterite_core::strata::*;\n",
        "use laterite_core::strata::*;",
        &resource_schema(&type_name, &table, &fields),
    )?;

    let store = src.join("store.rs");
    append_or_create(
        &store,
        "//! Queries over this plugin's tables.\n\nuse laterite_core::{strata::*, Db};\n",
        "use crate::schema::*;",
        &resource_store(&entity, &type_name, &fields),
    )?;

    let admin_dir = src.join("admin");
    fs::create_dir_all(&admin_dir)?;
    let admin = admin_dir.join(format!("{entity}.rs"));
    if admin.exists() {
        bail!("{} already exists", admin.display());
    }
    fs::write(
        &admin,
        resource_admin(&module_id, &entity, &table, &fields, source.as_ref()),
    )?;
    declare_module(&admin_dir.join("mod.rs"), &entity)?;

    // The crate root has to declare what was just created, or the command leaves
    // a crate that does not compile.
    wire_lib(&src.join("lib.rs"), &module_id)?;
    let registered = register_in_lib(&src.join("lib.rs"), &entity)?;

    // A sourced resource is a collection the site shows: a component for the
    // theme and the pages the build renders, one per record.
    let site_file = if let Some(shape) = &source {
        let site_dir = src.join("site");
        fs::create_dir_all(&site_dir)?;
        let file = site_dir.join(format!("{entity}.rs"));
        if file.exists() {
            bail!("{} already exists", file.display());
        }
        fs::write(
            &file,
            site_module(&module_id, &entity, &table, &fields, shape),
        )?;
        declare_site_module(&site_dir.join("mod.rs"), &entity)?;
        let placed = wire_site(&src.join("lib.rs"), &entity)?;
        add_dependencies(&args.path.join("Cargo.toml"))?;
        Some((file, placed))
    } else {
        None
    };

    println!("Created the {entity} resource:");
    if let Some(file) = migration {
        println!("  {}", file.display());
    }
    println!("  {}", admin.display());
    if let Some((file, _)) = &site_file {
        println!("  {}", file.display());
    }
    println!("  appended to {} and {}", schema.display(), store.display());
    if registered {
        println!("  registered in src/lib.rs");
    } else {
        println!("\nRegister it from your Module::register:");
        println!("  registry.add_resource(admin::{entity}::resource());");
        println!("  registry.add_permission(admin::{entity}::permission());");
    }
    if let Some(shape) = &source {
        let ext = match shape.format.as_str() {
            "markdown" => "md",
            other => other,
        };
        println!(
            "\nThe records live in {}/ as <slug>.{ext}; the folder is read when the application starts.",
            shape.path
        );
        println!(
            "The theme reads them through the `{module_id}.{table}` component; a page at /{table}/:slug is built per record."
        );
        if matches!(site_file, Some((_, false))) {
            println!("Call site::{entity}::register(registry) from your Module::register.");
        }
    }
    Ok(())
}

/// Adds `pub mod <entity>;` to `src/site/mod.rs`, creating it if absent.
fn declare_site_module(path: &Path, entity: &str) -> Result<()> {
    let mut contents = if path.is_file() {
        fs::read_to_string(path)?
    } else {
        "//! The site's side of this plugin: components the theme reads and the\n\
         //! pages the build renders.\n"
            .to_string()
    };
    let decl = format!("pub mod {entity};");
    if !contents.contains(&decl) {
        if !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(&format!("\n{decl}\n"));
    }
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Declares `mod site;` in the crate root and calls the entity's `register`
/// from the module's `register`, so the component and the pages are known.
/// Returns whether the call was placed; a crate root without the scaffolded
/// `register` gets the instruction printed instead.
fn wire_site(lib: &Path, entity: &str) -> Result<bool> {
    let Ok(mut src) = fs::read_to_string(lib) else {
        return Ok(false);
    };
    if !src.contains("mod site;") {
        let at = src.find("\nuse ").map(|i| i + 1).unwrap_or(src.len());
        src.insert_str(at, "mod site;\n\n");
    }
    let call = format!("        site::{entity}::register(registry);\n");
    let head = "    fn register(&self, registry: &mut Registry) {\n";
    let placed = if src.contains(&call) {
        true
    } else if let Some(i) = src.find(head) {
        src.insert_str(i + head.len(), &call);
        true
    } else {
        false
    };
    fs::write(lib, src).with_context(|| format!("writing {}", lib.display()))?;
    Ok(placed)
}

/// Adds the crates the site module uses to a plugin's manifest when absent.
fn add_dependencies(manifest: &Path) -> Result<()> {
    let Ok(mut toml) = fs::read_to_string(manifest) else {
        return Ok(());
    };
    let req = framework_requirement();
    let mut lines = Vec::new();
    if !toml.contains("\nlaterite-web") {
        lines.push(format!("laterite-web = \"{req}\""));
    }
    if !toml.contains("\nserde_json") {
        lines.push("serde_json = \"1\"".to_string());
    }
    if lines.is_empty() {
        return Ok(());
    }
    let header = "[dependencies]\n";
    match toml.find(header) {
        Some(i) => toml.insert_str(i + header.len(), &format!("{}\n", lines.join("\n"))),
        None => toml.push_str(&format!("\n[dependencies]\n{}\n", lines.join("\n"))),
    }
    fs::write(manifest, toml).with_context(|| format!("writing {}", manifest.display()))
}

/// The site module of a sourced resource: a component the theme reads the
/// records through, and the pages the build renders for them.
fn site_module(
    module_id: &str,
    entity: &str,
    table: &str,
    fields: &[Field],
    shape: &SourceShape,
) -> String {
    let code = if module_id.is_empty() {
        table.to_string()
    } else {
        format!("{module_id}.{table}")
    };
    let columns: String = fields
        .iter()
        .map(|f| format!("    \"{}\",\n", f.name))
        .collect();
    let switches = fields
        .iter()
        .filter(|f| f.kind == FieldKind::Boolean)
        .map(|f| format!("\"{}\"", f.name))
        .collect::<Vec<_>>()
        .join(", ");
    let plural_title = pascal(table);
    let body = if shape.markdown() {
        "Some(\"body\")"
    } else {
        "None"
    };
    format!(
        r#"//! The {table} on the site: the `{code}` component a page attaches,
//! and the pages the build renders, one per record at `/{table}/:slug`.
//!
//! ```yaml
//! # pages/{entity}.htm
//! url: /{table}/:slug
//! components:
//!   {entity}: {{ component: "{code}", slug: "{{{{ :slug }}}}" }}
//! ```

use std::sync::Arc;

use laterite_core::query::{{bind_values, build, text_cast, AnyRowExt}};
use laterite_core::strata::async_trait;
use laterite_core::{{Db, Registry}};
use laterite_web::theme::build::{{PathsReg, TableKeys}};
use laterite_web::theme::component::{{
    Component, ComponentContext, ComponentOutput, ComponentReg, PropertyDef, PropertyKind,
}};
use laterite_web::theme::ThemeError;
use sea_query::{{Alias, Expr, Order, Query}};
use serde_json::{{json, Map, Value}};

/// The columns a template reads, in the form's order.
const COLUMNS: &[&str] = &[
{columns}];

/// The switches, spelled as booleans.
const SWITCHES: &[&str] = &[{switches}];

/// The markdown column rendered to `body_html`, when there is one.
const BODY: Option<&str> = {body};

/// Registers the component and the pages the build renders.
pub fn register(registry: &mut Registry) {{
    registry.add(ComponentReg::new(
        "{code}",
        "{plural_title}",
        Arc::new({plural_title}),
    ));
    registry.add(PathsReg::new(
        "/{table}/:slug",
        Arc::new(TableKeys::new("{table}", "slug")),
    ));
}}

/// `{plural_title}`: every record newest first, or one by `slug`; `limit`
/// caps the list. Read as `<alias>.items` and `<alias>.item`.
pub struct {plural_title};

#[async_trait]
impl Component for {plural_title} {{
    fn properties(&self) -> Vec<PropertyDef> {{
        vec![
            PropertyDef::new("slug", "Slug", PropertyKind::Text).default(""),
            PropertyDef::new("limit", "How many", PropertyKind::Number).default(0),
        ]
    }}

    async fn run(&self, cx: &ComponentContext<'_>) -> Result<ComponentOutput, ThemeError> {{
        let slug = cx.properties["slug"].as_str().unwrap_or_default();
        let limit = cx.properties["limit"].as_u64().filter(|n| *n > 0);
        let items = rows(cx.db, slug, limit).await?;
        let item = items.first().cloned().unwrap_or(Value::Null);
        Ok(ComponentOutput::data(json!({{ "items": items, "item": item }})))
    }}
}}

/// The rows as a template sees them: every column as text, switches as
/// booleans, `url` added, and `body_html` when the body is markdown.
async fn rows(db: &Db, slug: &str, limit: Option<u64>) -> Result<Vec<Value>, ThemeError> {{
    // Built and dropped before the await: sea-query's identifiers are not `Send`.
    let (sql, values) = {{
        let cast = text_cast(db.backend);
        let mut select = Query::select();
        for column in COLUMNS {{
            select.expr_as(
                Expr::col(Alias::new(*column)).cast_as(Alias::new(cast)),
                Alias::new(*column),
            );
        }}
        select
            .from(Alias::new("{table}"))
            .order_by(Alias::new("created_at"), Order::Desc);
        if !slug.is_empty() {{
            select.and_where(Expr::col(Alias::new("slug")).eq(slug));
        }}
        if let Some(limit) = limit {{
            select.limit(limit);
        }}
        build(db.backend, select)
    }};
    let fetched = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await
        .map_err(|e| ThemeError::Config(e.to_string()))?;
    let mut out = Vec::with_capacity(fetched.len());
    for row in fetched {{
        let mut object = Map::new();
        for column in COLUMNS {{
            let text = row
                .get_text_opt(column)
                .map_err(|e| ThemeError::Config(e.to_string()))?;
            let value = match text {{
                None => Value::Null,
                Some(t) if SWITCHES.contains(column) => {{
                    Value::Bool(matches!(t.as_str(), "1" | "true"))
                }}
                Some(t) => Value::String(t),
            }};
            object.insert((*column).to_string(), value);
        }}
        let slug = object
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        object.insert("url".into(), json!(format!("/{table}/{{slug}}")));
        if let Some(body) = BODY {{
            let text = object
                .get(body)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            object.insert(
                "body_html".into(),
                json!(laterite_core::markdown::render(&text)),
            );
        }}
        out.push(Value::Object(object));
    }}
    Ok(out)
}}
"#
    )
}

/// Fills the register placeholders `make:plugin --admin` left, for the first
/// resource. Returns whether it did; a later resource is registered by hand.
fn register_in_lib(lib: &Path, entity: &str) -> Result<bool> {
    let Ok(src) = fs::read_to_string(lib) else {
        return Ok(false);
    };
    let placeholders = [
        "        // registry.add_permission(admin::thing::permission());\n",
        "        // registry.add_resource(admin::thing::resource());\n",
    ];
    if !placeholders.iter().all(|p| src.contains(p)) {
        return Ok(false);
    }
    let wired = src
        .replace(
            placeholders[0],
            &format!("        registry.add_permission(admin::{entity}::permission());\n"),
        )
        .replace(
            placeholders[1],
            &format!("        registry.add_resource(admin::{entity}::resource());\n"),
        )
        .replace("        let _ = registry;\n", "");
    fs::write(lib, wired).with_context(|| format!("writing {}", lib.display()))?;
    Ok(true)
}

/// Declares the modules a resource needs from the crate root, and points the
/// module's `migrations()` at the real set once there is one.
fn wire_lib(lib: &Path, module_id: &str) -> Result<()> {
    let Ok(mut src) = fs::read_to_string(lib) else {
        return Ok(());
    };
    let mut added = Vec::new();
    for decl in ["pub mod migrations;", "mod schema;", "pub mod store;"] {
        // `mod schema;` is a substring of nothing else here, but `pub mod store;`
        // would match a line already present, which is the point.
        if !src.contains(decl) {
            added.push(decl);
        }
    }
    if !added.is_empty() {
        let at = src.find("\nuse ").map(|i| i + 1).unwrap_or(src.len());
        src.insert_str(at, &format!("{}\n\n", added.join("\n")));
    }
    let empty = format!("MigrationSet::new(\"{module_id}\", Vec::new())");
    if src.contains(&empty) {
        src = src.replace(&empty, "migrations::migrations()");
        src = src.replace(
            "    /// None yet. `lat make:resource` creates `src/migrations/` with the first\n    /// one; point this at `migrations::migrations()` when it does.\n",
            "",
        );
    }
    fs::write(lib, src).with_context(|| format!("writing {}", lib.display()))
}

/// Appends to a file, creating it with `header` when absent and making sure
/// `needs` is imported either way.
///
/// The file may already exist from `make:plugin`, in which case its header was
/// written before this import was needed, so adding the line is not optional.
fn append_or_create(path: &Path, header: &str, needs: &str, body: &str) -> Result<()> {
    let mut contents = if path.is_file() {
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        header.to_string()
    };
    if !needs.is_empty() && !contents.contains(needs) {
        let after = contents
            .rfind("\nuse ")
            .and_then(|i| contents[i + 1..].find('\n').map(|j| i + 1 + j + 1))
            .unwrap_or(contents.len());
        contents.insert_str(after, &format!("{needs}\n"));
    }
    // A scaffolded stub carries an `#[allow(unused_imports)]` that stops being
    // true the moment real code lands beside it.
    contents = contents.replace("#[allow(unused_imports)]\n", "");
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(body);
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Adds `mod <name>;` to a module file, creating it if absent.
fn declare_module(path: &Path, name: &str) -> Result<()> {
    let mut contents = if path.is_file() {
        fs::read_to_string(path)?
    } else {
        "//! This plugin's admin surface: descriptors, screens and settings.\n".to_string()
    };
    contents = contents.replace(
        "#[allow(unused_imports)]\nuse laterite_admin::{list::ListConfig, Permission, Resource};\n",
        "",
    );
    let decl = format!("pub mod {name};");
    if !contents.contains(&decl) {
        if !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(&format!("\n{decl}\n"));
    }
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// `canonical_name` -> `Canonical name`: a label a person reads, not an
/// identifier. Only the first word is capitalised, as a column heading is.
fn label(snake: &str) -> String {
    let mut words = snake.split('_').filter(|w| !w.is_empty());
    let Some(first) = words.next() else {
        return String::new();
    };
    let mut out = pascal(first);
    for word in words {
        out.push(' ');
        out.push_str(word);
    }
    out
}

/// `article` -> `Article`.
fn pascal(snake: &str) -> String {
    snake
        .split('_')
        .filter(|p| !p.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// A serviceable English plural. Wrong for irregular nouns, and an author
/// renaming the table is cheaper than this command guessing at grammar.
fn plural(word: &str) -> String {
    if word.ends_with('s') || word.ends_with("ch") || word.ends_with("sh") || word.ends_with('x') {
        format!("{word}es")
    } else if word.ends_with('y')
        && !word.ends_with("ay")
        && !word.ends_with("ey")
        && !word.ends_with("oy")
    {
        format!("{}ies", &word[..word.len() - 1])
    } else {
        format!("{word}s")
    }
}

fn resource_migration(name: &str, table: &str, type_name: &str, fields: &[Field]) -> String {
    let columns: String = fields
        .iter()
        .map(|f| {
            format!(
                "                .col({})\n",
                f.kind.column(&format!("{type_name}::{}", pascal(&f.name)))
            )
        })
        .collect();
    format!(
        r#"//! Create the {table} table.

use laterite_core::strata::*;

use crate::schema::{type_name};

pub struct Migration;

#[async_trait(?Send)]
impl laterite_core::Migration for Migration {{
    fn name(&self) -> &str {{
        "{name}"
    }}

    async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {{
        s.exec(
            Table::create()
                .table({type_name}::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new({type_name}::Id)
                        .big_integer()
                        .not_null()
                        .auto_increment()
                        .primary_key(),
                )
{columns}                .col(ColumnDef::new({type_name}::CreatedAt).text().not_null())
                .col(ColumnDef::new({type_name}::UpdatedAt).text().not_null())
                .to_owned(),
        )
        .await
    }}

    async fn down(&self, s: &mut Schema<'_>) -> CoreResult<()> {{
        s.exec(Table::drop().table({type_name}::Table).to_owned()).await
    }}
}}
"#
    )
}

fn resource_schema(type_name: &str, table: &str, fields: &[Field]) -> String {
    let columns: String = fields
        .iter()
        .map(|f| format!("    {},\n", pascal(&f.name)))
        .collect();
    format!(
        r#"
/// The `{table}` table.
#[derive(Iden)]
pub enum {type_name} {{
    #[iden = "{table}"]
    Table,
    Id,
{columns}    CreatedAt,
    UpdatedAt,
}}
"#
    )
}

fn resource_store(entity: &str, type_name: &str, fields: &[Field]) -> String {
    let struct_fields: String = fields
        .iter()
        .map(|f| {
            let ty = match f.kind {
                FieldKind::Text | FieldKind::DateTime => "String",
                FieldKind::Integer => "i64",
                FieldKind::Boolean => "bool",
            };
            format!("    pub {}: {ty},\n", f.name)
        })
        .collect();
    let reads: String = fields
        .iter()
        .map(|f| {
            let getter = match f.kind {
                FieldKind::Text | FieldKind::DateTime => "get_text",
                FieldKind::Integer => "get_int",
                FieldKind::Boolean => "get_bool",
            };
            format!("            {}: r.{getter}(\"{}\")?,\n", f.name, f.name)
        })
        .collect();
    let columns: String = fields
        .iter()
        .map(|f| format!("                {type_name}::{},\n", pascal(&f.name)))
        .collect();
    let name = pascal(entity);
    format!(
        r#"
/// One row of the `{type_name}` table.
#[derive(Debug, Clone)]
pub struct {name} {{
    pub id: i64,
{struct_fields}}}

/// Reads one row by id, or `None` when it is gone.
pub async fn find_{entity}(db: &Db, id: i64) -> CoreResult<Option<{name}>> {{
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                {type_name}::Id,
{columns}            ])
            .from({type_name}::Table)
            .and_where(Expr::col({type_name}::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.map(|r| {{
        Ok({name} {{
            id: r.get_int("id")?,
{reads}        }})
    }})
    .transpose()
}}
"#
    )
}

fn resource_admin(
    module_id: &str,
    entity: &str,
    table: &str,
    fields: &[Field],
    source: Option<&SourceShape>,
) -> String {
    // An application crate has no module marker; its permissions are its own.
    let permission = if module_id.is_empty() {
        format!("{entity}.manage")
    } else {
        format!("{module_id}.{entity}.manage")
    };
    let title = pascal(entity);
    let plural_title = pascal(table);
    let list_columns: String = fields
        .iter()
        .map(|f| {
            format!(
                "            ListColumn::new(\"{}\", \"{}\"){},\n",
                f.name,
                label(&f.name),
                f.kind.column_type()
            )
        })
        .collect();
    let form_fields: String = fields
        .iter()
        .map(|f| {
            let constructor = match (source, f.name.as_str()) {
                (Some(shape), "body") if shape.markdown() => "textarea",
                _ => f.kind.form_field(),
            };
            let rules = match (source, f.name.as_str()) {
                (Some(_), "slug") => ".unique()",
                _ => "",
            };
            format!(
                "            FormField::{constructor}(\"{}\", \"{}\"){rules},\n",
                f.name,
                label(&f.name)
            )
        })
        .collect();
    let (source_use, source_call) = match source {
        Some(shape) => {
            let constructor = if shape.git { "git" } else { "files" };
            let format = match shape.format.as_str() {
                "json" => "Json",
                "markdown" => "Markdown",
                _ => "Yaml",
            };
            let body = if shape.markdown() {
                "\n                .body(\"body\")"
            } else {
                ""
            };
            let slug_from = fields
                .iter()
                .find(|f| f.kind == FieldKind::Text && f.name != "slug" && f.name != "body")
                .map(|f| format!("\n                .slug_from(\"{}\")", f.name))
                .unwrap_or_default();
            (
                "use laterite_core::source::{Format, Source};\n".to_string(),
                format!(
                    "        .source(\n            Source::{constructor}(\"{}\", Format::{format}){body}{slug_from},\n        )\n",
                    shape.path
                ),
            )
        }
        None => (String::new(), String::new()),
    };
    format!(
        r#"//! The {table} admin screen: a list and a create/edit form.

use laterite_admin::form::{{FormConfig, FormField}};
use laterite_admin::list::{{ListColumn, ListConfig}};
use laterite_admin::{{Permission, Resource}};
{source_use}
/// The permission gating this resource. A resource with a form must declare one,
/// or boot aborts.
pub const MANAGE: &str = "{permission}";

pub fn permission() -> Permission {{
    Permission::new(MANAGE, "Manage {table}", "{plural_title}")
        .roles([laterite_admin::ROLE_ADMIN, laterite_admin::ROLE_EDITOR])
}}

pub fn resource() -> Resource {{
    Resource::new("/{table}", "{plural_title}", list())
        .form(form())
        .permission(MANAGE)
{source_call}}}

fn list() -> ListConfig {{
    ListConfig::new(
        "{table}",
        "{plural_title}",
        vec![
{list_columns}            ListColumn::new("created_at", "Created").datetime(),
        ],
    )
    .edit_base("/{table}")
    .creatable()
    // No `.deletable()`: removing a record is a decision, not a default. Add it
    // once you know nothing depends on these rows.
}}

fn form() -> FormConfig {{
    FormConfig::new(
        "{table}",
        "{title}",
        "/{table}",
        "id",
        vec![
{form_fields}        ],
    )
    .timestamps()
}}
"#
    )
}
