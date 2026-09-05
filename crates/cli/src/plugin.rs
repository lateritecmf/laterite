//! `lat plugin`: discover the plugins under `plugins/<author>/<plugin>/` and keep
//! the generated `plugins-manifest` crate in step with them.
//!
//! Plugin code is linked at build time, so a single binary cannot pick up a
//! plugin folder at runtime the way a scripting CMS does. Instead `sync` scans
//! the plugin tree, checks each crate against its folder, and regenerates the
//! `plugins-manifest` crate that `Bootstrap::modules` reads. Dropping a plugin in
//! and running `lat plugin sync` (then rebuilding) is the compiled equivalent of
//! a drop-in install.
//!
//! Each plugin crate exposes `pub fn module() -> Box<dyn Module>` as its entry
//! point, so the generated manifest collects it without knowing the type name.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::Deserialize;

/// Where plugins live, relative to the application root.
const PLUGINS_DIR: &str = "plugins";
/// The generated aggregator crate, a sibling of `plugins/`.
const MANIFEST_DIR: &str = "plugins-manifest";

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Regenerate the plugins-manifest crate from the plugins/ tree.
    Sync,
    /// List the plugins discovered under plugins/.
    List,
}

pub fn run(command: PluginCommand) -> Result<()> {
    let project = crate::project::Project::locate()?;
    match command {
        PluginCommand::Sync => sync(&project.root),
        PluginCommand::List => list(&project.root),
    }
}

/// A plugin discovered from the `plugins/<author>/<plugin>/` layout, its crate
/// name verified against the folder names.
#[derive(Debug)]
struct Plugin {
    author: String,
    name: String,
    crate_name: String,
}

impl Plugin {
    /// The crate identifier in Rust paths (`rainmill-location` -> `rainmill_location`).
    fn ident(&self) -> String {
        self.crate_name.replace('-', "_")
    }

    /// The runtime module id the folder layout implies (`rainmill.location`).
    fn module_id(&self) -> String {
        format!("{}.{}", self.author, self.name)
    }

    /// The dependency path from the manifest crate to this plugin.
    fn dep_path(&self) -> String {
        format!("../{PLUGINS_DIR}/{}/{}", self.author, self.name)
    }
}

/// The `[package]` slice of a plugin's Cargo.toml that `sync` needs.
#[derive(Deserialize)]
struct Manifest {
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    name: String,
}

fn sync(root: &Path) -> Result<()> {
    let plugins = discover(&root.join(PLUGINS_DIR))?;
    write_manifest(&root.join(MANIFEST_DIR), &plugins)?;
    if plugins.is_empty() {
        println!("No plugins under {PLUGINS_DIR}/; wrote an empty {MANIFEST_DIR}.");
    } else {
        println!("Synced {} plugin(s) into {MANIFEST_DIR}/:", plugins.len());
        for p in &plugins {
            println!("  {} ({})", p.module_id(), p.crate_name);
        }
    }
    Ok(())
}

fn list(root: &Path) -> Result<()> {
    let plugins = discover(&root.join(PLUGINS_DIR))?;
    if plugins.is_empty() {
        println!("No plugins under {PLUGINS_DIR}/.");
        return Ok(());
    }
    for p in &plugins {
        println!(
            "{:<24} {:<24} {}",
            p.module_id(),
            p.crate_name,
            p.dep_path()
        );
    }
    Ok(())
}

/// Scans `<root>/<author>/<plugin>/` for plugin crates, checking each crate name
/// matches its folder (`author-plugin`). Returns them sorted by author then name,
/// so the generated manifest is stable across runs and machines.
fn discover(root: &Path) -> Result<Vec<Plugin>> {
    if !root.is_dir() {
        bail!(
            "no {}/ directory here; run this from a Laterite application that uses the plugin layout",
            root.display()
        );
    }
    let mut plugins = Vec::new();
    let mut problems = Vec::new();
    for author_dir in subdirs(root)? {
        let author = file_name(&author_dir);
        for plugin_dir in subdirs(&author_dir)? {
            let manifest = plugin_dir.join("Cargo.toml");
            if !manifest.is_file() {
                continue; // not a crate; ignore stray directories
            }
            let name = file_name(&plugin_dir);
            let crate_name = read_crate_name(&manifest)?;
            let expected = format!("{author}-{name}");
            if crate_name != expected {
                problems.push(format!(
                    "  {}: crate is '{crate_name}', but the {PLUGINS_DIR}/{author}/{name} layout requires '{expected}'",
                    plugin_dir.display()
                ));
                continue;
            }
            plugins.push(Plugin {
                author: author.clone(),
                name,
                crate_name,
            });
        }
    }
    if !problems.is_empty() {
        bail!(
            "plugin crate names must match their folders (crate = author-plugin):\n{}",
            problems.join("\n")
        );
    }
    Ok(plugins)
}

fn write_manifest(dir: &Path, plugins: &[Plugin]) -> Result<()> {
    fs::create_dir_all(dir.join("src"))
        .with_context(|| format!("creating {}/src", dir.display()))?;
    fs::write(dir.join("Cargo.toml"), manifest_cargo(plugins))
        .with_context(|| format!("writing {}/Cargo.toml", dir.display()))?;
    fs::write(dir.join("src/lib.rs"), manifest_lib(plugins))
        .with_context(|| format!("writing {}/src/lib.rs", dir.display()))?;
    Ok(())
}

fn manifest_cargo(plugins: &[Plugin]) -> String {
    let mut deps = String::from("laterite-core = { workspace = true }\n");
    for p in plugins {
        deps.push_str(&format!(
            "{} = {{ path = \"{}\" }}\n",
            p.crate_name,
            p.dep_path()
        ));
    }
    format!(
        r#"# @generated by `lat plugin sync` - do not edit.
# Regenerate after adding or removing a plugin under {PLUGINS_DIR}/.
[package]
name = "plugins-manifest"
version = "0.1.0"
edition.workspace = true
publish = false

[dependencies]
{deps}"#
    )
}

fn manifest_lib(plugins: &[Plugin]) -> String {
    let body = if plugins.is_empty() {
        "    vec![]".to_string()
    } else {
        let calls: String = plugins
            .iter()
            .map(|p| format!("        {}::module(),\n", p.ident()))
            .collect();
        format!("    vec![\n{calls}    ]")
    };
    format!(
        r#"// @generated by `lat plugin sync` - do not edit.
//
// Every plugin under {PLUGINS_DIR}/<author>/<plugin>/, aggregated into `all()` for
// `Bootstrap::modules`. Each plugin crate exposes `module()`. Regenerate with
// `lat plugin sync`.

use laterite_core::Module;

// One module per line and generated verbatim, so `cargo fmt` leaves it as `sync`
// wrote it and the drift check stays a plain comparison.
/// Every installed plugin, in a stable order (vendor then plugin).
#[rustfmt::skip]
pub fn all() -> Vec<Box<dyn Module>> {{
{body}
}}
"#
    )
}

/// Whether the generated manifest matches the current plugin tree under the
/// application at `app_root`. `None` when the app doesn't use the plugin layout
/// (no plugins/ dir), so `doctor` can skip the check.
pub fn manifest_in_sync(app_root: &Path) -> Result<Option<bool>> {
    let root = app_root.join(PLUGINS_DIR);
    if !root.is_dir() {
        return Ok(None);
    }
    let plugins = discover(&root)?;
    let dir = app_root.join(MANIFEST_DIR);
    let cargo_ok =
        fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default() == manifest_cargo(&plugins);
    let lib_ok =
        fs::read_to_string(dir.join("src/lib.rs")).unwrap_or_default() == manifest_lib(&plugins);
    Ok(Some(cargo_ok && lib_ok))
}

fn read_crate_name(manifest: &Path) -> Result<String> {
    let text =
        fs::read_to_string(manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let parsed: Manifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest.display()))?;
    Ok(parsed.package.name)
}

/// The immediate sub-directories of `dir`, sorted by name. Symlinked directories
/// (the dev layout for a plugin checkout) are followed.
fn subdirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(root: &Path, author: &str, name: &str, crate_name: &str) {
        let dir = root.join(author).join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn discover_finds_and_sorts_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_plugin(root, "acme", "shop", "acme-shop");
        write_plugin(root, "acme", "blog", "acme-blog");
        let plugins = discover(root).unwrap();
        assert_eq!(plugins.len(), 2);
        // Sorted, so the generated manifest is stable: acme/blog before acme/shop.
        assert_eq!(plugins[0].module_id(), "acme.blog");
        assert_eq!(plugins[1].module_id(), "acme.shop");
    }

    #[test]
    fn discover_rejects_a_crate_name_that_mismatches_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "acme", "blog", "wrong-name");
        let err = discover(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("acme-blog"), "{err}");
    }

    #[test]
    fn discover_ignores_non_crate_directories() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "acme", "blog", "acme-blog");
        // A directory without a Cargo.toml is not a plugin crate.
        fs::create_dir_all(tmp.path().join("acme").join("assets")).unwrap();
        assert_eq!(discover(tmp.path()).unwrap().len(), 1);
    }

    #[test]
    fn manifest_lib_lists_each_plugins_entry_point() {
        let plugins = vec![
            Plugin {
                author: "acme".into(),
                name: "blog".into(),
                crate_name: "acme-blog".into(),
            },
            Plugin {
                author: "rainmill".into(),
                name: "location".into(),
                crate_name: "rainmill-location".into(),
            },
        ];
        let lib = manifest_lib(&plugins);
        assert!(lib.contains("acme_blog::module(),"));
        assert!(lib.contains("rainmill_location::module(),"));
        assert!(lib.contains("pub fn all() -> Vec<Box<dyn Module>>"));
        assert!(lib.contains("@generated"));
    }

    #[test]
    fn manifest_cargo_declares_each_plugin_by_path() {
        let plugins = vec![Plugin {
            author: "rainmill".into(),
            name: "location".into(),
            crate_name: "rainmill-location".into(),
        }];
        let cargo = manifest_cargo(&plugins);
        assert!(cargo.contains(r#"rainmill-location = { path = "../plugins/rainmill/location" }"#));
        assert!(cargo.contains("laterite-core = { workspace = true }"));
    }

    #[test]
    fn an_empty_tree_generates_an_empty_manifest() {
        assert!(manifest_lib(&[]).contains("vec![]"));
    }
}
