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
