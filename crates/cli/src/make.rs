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
    name: String,
    /// The crate to scaffold in. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    path: PathBuf,
    /// Columns beyond the id and timestamps, as `name:type`. Types: text,
    /// integer, boolean, datetime. Repeat the flag or comma-separate.
    #[arg(long, value_delimiter = ',')]
    field: Vec<String>,
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
            Self::Boolean => format!("bool_col({ident}).not_null().default(false)"),
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

    let fields: Vec<Field> = args
        .field
        .iter()
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
        .collect::<Result<_>>()?;

    let table = plural(&entity);
    // The table's identifier enum is named for the table, the row struct for the
    // entity, so `Articles::Title` is a column and `Article` is one row. Naming
    // both after the entity makes `Article::Id` ambiguous.
    let type_name = pascal(&table);

    // The migration, through the same path `make:migration` takes, so both
    // commands agree on numbering and on listing it in the manifest.
    let migrations_dir = src.join("migrations");
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
    fs::write(&admin, resource_admin(&entity, &table, &fields))?;
    declare_module(&admin_dir.join("mod.rs"), &entity)?;

    println!("Created the {entity} resource:");
    if let Some(file) = migration {
        println!("  {}", file.display());
    }
    println!("  {}", admin.display());
    println!("  appended to {} and {}", schema.display(), store.display());
    println!("\nRegister it from your Module::register:");
    println!("  registry.add_resource(admin::{entity}::resource());");
    println!("  registry.add_permission(admin::{entity}::permission());");
    Ok(())
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

fn resource_admin(entity: &str, table: &str, fields: &[Field]) -> String {
    let title = pascal(entity);
    let plural_title = pascal(table);
    let list_columns: String = fields
        .iter()
        .map(|f| {
            format!(
                "            ListColumn::new(\"{}\", \"{}\"){},\n",
                f.name,
                pascal(&f.name),
                f.kind.column_type()
            )
        })
        .collect();
    let form_fields: String = fields
        .iter()
        .map(|f| {
            format!(
                "            FormField::{}(\"{}\", \"{}\"),\n",
                f.kind.form_field(),
                f.name,
                pascal(&f.name)
            )
        })
        .collect();
    format!(
        r#"//! The {table} admin screen: a list and a create/edit form.

use laterite_admin::form::{{FormConfig, FormField}};
use laterite_admin::list::{{ListColumn, ListConfig}};
use laterite_admin::{{Permission, Resource}};

/// The permission gating this resource. A resource with a form must declare one,
/// or boot aborts.
pub const MANAGE: &str = "{entity}.manage";

pub fn permission() -> Permission {{
    Permission {{
        code: MANAGE.to_string(),
        label: "Manage {table}".into(),
        group: "{plural_title}".into(),
    }}
}}

pub fn resource() -> Resource {{
    Resource::new("/{table}", "{plural_title}", list())
        .form(form())
        .permission(MANAGE)
}}

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
    .deletable()
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
