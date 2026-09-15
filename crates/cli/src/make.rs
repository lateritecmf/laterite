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
    // The manifest holds a single `migration_set!` block whose only bare `}`
    // line closes it; insert the entry just above that line.
    let close = lines
        .iter()
        .rposition(|l| l.trim() == "}")
        .context("could not find the migration_set! block to extend")?;
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
    id: String,
    /// Where to create it. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    path: PathBuf,
    /// Scaffold the admin half (descriptors, screens, settings).
    #[arg(long)]
    admin: bool,
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

    fs::create_dir_all(root.join("src/migrations"))?;
    write(
        &root.join("Cargo.toml"),
        &plugin_cargo(&crate_name, &args.id),
    )?;
    write(&root.join("src/lib.rs"), &plugin_lib(&args.id, args.admin))?;
    write(
        &root.join("src/migrations/mod.rs"),
        &plugin_migrations(&args.id),
    )?;
    write(&root.join("src/schema.rs"), PLUGIN_SCHEMA)?;
    write(&root.join("src/store.rs"), PLUGIN_STORE)?;
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

fn plugin_cargo(crate_name: &str, id: &str) -> String {
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

[dependencies]
# Version-only, so this manifest does not depend on where the crate sits. An
# application building against a local framework checkout resolves these through
# its own [patch.crates-io].
laterite-core = "0.5"
laterite-admin = "0.5"
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
laterite-core = {{ version = "0.5", features = ["sqlite", "testing"] }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
"#
    )
}

fn plugin_lib(id: &str, admin: bool) -> String {
    let admin_mod = if admin { "mod admin;\n" } else { "" };
    let register = if admin {
        "\n    fn register(&self, registry: &mut Registry) {\n        \
         use laterite_admin::AdminRegistry;\n        let _ = registry;\n        \
         // registry.add_permission(admin::permission());\n        \
         // registry.add_resource(admin::resource());\n    }\n"
    } else {
        ""
    };
    format!(
        r#"//! A Laterite plugin.

{admin_mod}pub mod migrations;
mod schema;
pub mod store;

use laterite_core::{{MigrationSet, Module, ModuleId, Registry}};

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

    fn migrations(&self) -> MigrationSet {{
        migrations::migrations()
    }}
{register}}}
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

const PLUGIN_SCHEMA: &str = r#"//! Table and column identifiers for this plugin's schema.
//!
//! One `Iden` enum per table, so a query names a column rather than spelling it.

#[allow(unused_imports)]
use laterite_core::strata::*;
"#;

const PLUGIN_STORE: &str = r#"//! Queries over this plugin's tables.
//!
//! Built with sea-query through the portable helpers, so they run on every
//! supported database. Nothing here builds SQL from a string.

#[allow(unused_imports)]
use laterite_core::{strata::*, Db};
"#;

const PLUGIN_ADMIN: &str = r#"//! This plugin's admin surface: descriptors, screens and settings.
//!
//! Screens are data. Reach for a `Resource` first and a `Screen` only for what a
//! list and a form cannot express.

#[allow(unused_imports)]
use laterite_admin::{list::ListConfig, Permission, Resource};
"#;
