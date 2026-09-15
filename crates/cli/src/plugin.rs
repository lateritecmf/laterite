//! `lat plugin`: the plugins this application compiles in, and the generated
//! `plugins-manifest` crate that links them.
//!
//! Plugin code is linked at build time, so a single binary cannot pick up a
//! folder at runtime the way a scripting CMS does. What is compiled in is
//! therefore a build-time fact, recorded in `plugins/plugins.toml`; which of
//! those are *active* is runtime state, held in the database and toggled from
//! the admin. Two kinds of truth, each owned by the thing that can honour it.
//!
//! `add` and `remove` edit the list, `sync` regenerates the manifest crate from
//! it, and a rebuild picks the change up. Folder names carry no meaning: the
//! list says where each plugin is, and the plugin's own `Cargo.toml` says what
//! it is called, so nothing has to be renamed to be installed.
//!
//! Each plugin crate exposes `pub fn module() -> Box<dyn Module>` at its root,
//! so the generated manifest collects it without knowing the type name.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::Deserialize;

/// Where plugins live, relative to the application root.
const PLUGINS_DIR: &str = "plugins";
/// The declarative list of compiled-in plugins, inside `plugins/`.
const LIST_FILE: &str = "plugins.toml";
/// The generated aggregator crate, a sibling of `plugins/`.
const MANIFEST_DIR: &str = "plugins-manifest";

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Add a plugin from a local path or a git URL, and record it.
    Add {
        /// A local directory, or a git URL to clone into plugins/.
        source: String,
        /// Folder name to use under plugins/. Defaults to the crate's own name.
        #[arg(long)]
        r#as: Option<String>,
    },
    /// Remove a plugin from the list by crate name or folder.
    Remove {
        /// The plugin's crate name (`rainmill-discovery`) or its folder.
        name: String,
        /// Also delete the plugin's folder from plugins/.
        #[arg(long)]
        delete: bool,
    },
    /// Regenerate the plugins-manifest crate from plugins/plugins.toml.
    Sync,
    /// List the plugins this application compiles in.
    List,
    /// Check each installed plugin against the expected layout.
    Check,
}

pub fn run(command: PluginCommand) -> Result<()> {
    let project = crate::project::Project::locate()?;
    match command {
        PluginCommand::Add { source, r#as } => add(&project.root, &source, r#as.as_deref()),
        PluginCommand::Remove { name, delete } => remove(&project.root, &name, delete),
        PluginCommand::Sync => sync(&project.root),
        PluginCommand::List => list(&project.root),
        PluginCommand::Check => check(&project.root),
    }
}

/// One entry in `plugins/plugins.toml`.
#[derive(Debug, Clone, Deserialize)]
struct Entry {
    /// Where the plugin crate is, relative to `plugins/`.
    path: String,
    /// Where it came from, when it was cloned. Provenance only; nothing resolves
    /// it, so a plugin vendored by hand simply has none.
    #[serde(default)]
    source: Option<String>,
}

/// A resolved entry: its recorded path plus the crate name its own manifest
/// declares. The crate name is never stored in the list, so the two can never
/// disagree.
#[derive(Debug)]
struct Plugin {
    entry: Entry,
    crate_name: String,
}

impl Plugin {
    /// The crate identifier in Rust paths (`rainmill-location` -> `rainmill_location`).
    fn ident(&self) -> String {
        self.crate_name.replace('-', "_")
    }

    /// The dependency path from the manifest crate to this plugin.
    fn dep_path(&self) -> String {
        format!("../{PLUGINS_DIR}/{}", self.entry.path)
    }
}

/// The slices of a Cargo.toml this command reads. Everything here is available
/// without building the crate, which is what makes the checks an upfront gate
/// rather than a compile error later.
#[derive(Deserialize)]
struct CargoManifest {
    package: Package,
    #[serde(default)]
    dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    workspace: Option<Workspace>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    /// Absent when the crate inherits it from its workspace, which a framework
    /// crate does not.
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    metadata: Option<Metadata>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    laterite: Option<LateriteMetadata>,
}

/// What a crate declares about itself as a plugin.
///
/// Its presence is the marker: a crate without it is not a plugin, and `add`
/// says so rather than letting the build fail on a missing `module()`.
/// Deliberately **not** a version field. Which Laterite a plugin supports is
/// already stated by its dependency requirement, and a second copy of that fact
/// could disagree with the first.
#[derive(Deserialize)]
struct LateriteMetadata {
    /// The module id this crate registers (`vendor.package`), matching its
    /// `Module::id()`. The marketplace keys on it, and it is what a person
    /// installing by name asked for.
    plugin: String,
}

#[derive(Deserialize)]
struct Workspace {
    #[serde(default)]
    dependencies: BTreeMap<String, Dependency>,
}

/// A dependency as Cargo writes it: a bare version, or a table.
#[derive(Deserialize)]
#[serde(untagged)]
enum Dependency {
    Version(String),
    Table {
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
}

impl Dependency {
    fn version(&self) -> Option<&str> {
        match self {
            Dependency::Version(v) => Some(v),
            Dependency::Table { version, .. } => version.as_deref(),
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Dependency::Version(_) => None,
            Dependency::Table { path, .. } => path.as_deref(),
        }
    }
}

/// The framework crates a plugin may depend on. Any one of them states which
/// Laterite it was built for, since they release in lockstep.
const FRAMEWORK_CRATES: [&str; 4] = [
    "laterite-admin",
    "laterite-core",
    "laterite-auth",
    "laterite-web",
];

/// The whole list file.
#[derive(Default, Deserialize)]
struct PluginList {
    #[serde(default, rename = "plugin")]
    plugins: Vec<Entry>,
}

// -------------------------------------------------------- compatibility

fn read_manifest(path: &Path) -> Result<CargoManifest> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Which Laterite a crate was built for, read from whichever framework crate it
/// depends on: they release in lockstep, so any one of them says it.
///
/// `None` means the crate names no framework crate by version at all, which for
/// a plugin means a local checkout (a path dependency) and nothing to check.
fn framework_requirement(manifest: &CargoManifest) -> Option<(String, semver::VersionReq)> {
    for name in FRAMEWORK_CRATES {
        if let Some(raw) = manifest
            .dependencies
            .get(name)
            .and_then(Dependency::version)
        {
            if let Ok(req) = semver::VersionReq::parse(raw) {
                return Some((name.to_string(), req));
            }
        }
    }
    None
}

/// The Laterite version this application builds against.
///
/// Read from its own manifest rather than from the running `lat`, because the
/// two can differ: a command installed from one checkout may be run inside an
/// application pinned to another. A path dependency (a local framework checkout)
/// is followed to that crate's own version, so development mode is checked as
/// accurately as a published one.
fn app_framework_version(root: &Path) -> Option<semver::Version> {
    let manifest = read_manifest(&root.join("Cargo.toml")).ok()?;
    let tables = [
        Some(&manifest.dependencies),
        manifest.workspace.as_ref().map(|w| &w.dependencies),
    ];
    for table in tables.into_iter().flatten() {
        for name in FRAMEWORK_CRATES {
            let Some(dep) = table.get(name) else { continue };
            if let Some(path) = dep.path() {
                let at = root.join(path).join("Cargo.toml");
                if let Some(version) = read_manifest(&at)
                    .ok()
                    .and_then(|m| m.package.version)
                    .and_then(|v| semver::Version::parse(&v).ok())
                {
                    return Some(version);
                }
            }
            // A requirement is not a version, so take the lowest release it
            // admits: `"0.5"` means this application is on some 0.5.x.
            if let Some(req) = dep
                .version()
                .and_then(|v| semver::VersionReq::parse(v).ok())
            {
                if let Some(c) = req.comparators.first() {
                    return Some(semver::Version::new(
                        c.major,
                        c.minor.unwrap_or(0),
                        c.patch.unwrap_or(0),
                    ));
                }
            }
        }
    }
    None
}

/// Refuses a crate that is not a Laterite plugin, or one built for a Laterite
/// this application is not on.
///
/// Both are knowable from the crate's manifest without building it, which is the
/// point: the alternative is a compile error about a missing `module()`, or a
/// dependency resolution failure, neither of which names the actual problem.
/// Boot stays the backstop; this is the earlier, friendlier gate, matching how
/// a plugin's database capabilities are checked.
fn check_compatible(root: &Path, manifest: &CargoManifest, crate_name: &str) -> Result<String> {
    let Some(declared) = manifest
        .package
        .metadata
        .as_ref()
        .and_then(|m| m.laterite.as_ref())
    else {
        bail!(
            "{crate_name} is not a Laterite plugin: its Cargo.toml has no \
             [package.metadata.laterite] section.\n\
             A plugin declares the module id it registers there:\n\n    \
             [package.metadata.laterite]\n    plugin = \"vendor.package\""
        );
    };

    if let (Some((named, req)), Some(version)) =
        (framework_requirement(manifest), app_framework_version(root))
    {
        if !req.matches(&version) {
            bail!(
                "{crate_name} needs {named} {req}, and this application is on {version}.\n\
                 Look for a release of the plugin that supports {}.{}, or upgrade this \
                 application to one it supports.",
                version.major,
                version.minor
            );
        }
    }
    Ok(declared.plugin.clone())
}

// ---------------------------------------------------------------- the list

fn list_path(root: &Path) -> PathBuf {
    root.join(PLUGINS_DIR).join(LIST_FILE)
}

/// Reads `plugins/plugins.toml`, resolving each entry against its crate manifest.
///
/// An application with no `plugins/` directory is not using plugins at all, and
/// gets `None` so callers can stay quiet rather than reporting a problem.
fn read_list(root: &Path) -> Result<Option<Vec<Plugin>>> {
    let dir = root.join(PLUGINS_DIR);
    if !dir.is_dir() {
        return Ok(None);
    }
    let file = list_path(root);
    let entries = if file.is_file() {
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let parsed: PluginList =
            toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
        parsed.plugins
    } else {
        // An application from before the list existed has its plugins in the old
        // `<author>/<plugin>` layout. Adopt them once rather than reporting none,
        // so an upgrade needs no hand-editing.
        let found = scan_legacy(&dir)?;
        if !found.is_empty() {
            write_list(root, &found)?;
            println!(
                "Adopted {} plugin(s) from the folder layout into {PLUGINS_DIR}/{LIST_FILE}.",
                found.len()
            );
        }
        found
    };

    let mut plugins = Vec::new();
    for entry in entries {
        let crate_dir = dir.join(&entry.path);
        let manifest = crate_dir.join("Cargo.toml");
        if !manifest.is_file() {
            bail!(
                "{PLUGINS_DIR}/{LIST_FILE} lists '{}', but {} has no Cargo.toml.\n\
                 Fix the path, or drop it with `lat plugin remove {}`.",
                entry.path,
                crate_dir.display(),
                entry.path
            );
        }
        let crate_name = read_crate_name(&manifest)?;
        plugins.push(Plugin { entry, crate_name });
    }
    // Sorted by crate name so the generated manifest is stable across machines.
    plugins.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    if let Some(pair) = plugins
        .windows(2)
        .find(|w| w[0].crate_name == w[1].crate_name)
    {
        bail!(
            "{PLUGINS_DIR}/{LIST_FILE} lists {} twice, at '{}' and '{}'. A plugin can \
             only be compiled in once.",
            pair[0].crate_name,
            pair[0].entry.path,
            pair[1].entry.path
        );
    }
    Ok(Some(plugins))
}

/// The pre-list layout: `plugins/<author>/<plugin>/`. Read once, to migrate.
fn scan_legacy(dir: &Path) -> Result<Vec<Entry>> {
    let mut found = Vec::new();
    for author in subdirs(dir)? {
        for plugin in subdirs(&author)? {
            if plugin.join("Cargo.toml").is_file() {
                found.push(Entry {
                    path: format!("{}/{}", file_name(&author), file_name(&plugin)),
                    source: None,
                });
            }
        }
    }
    Ok(found)
}

/// Writes the list. Hand-formatted rather than serialized, because this file is
/// read by people and the comment at the top is the point.
fn write_list(root: &Path, entries: &[Entry]) -> Result<()> {
    let mut out = String::from(
        "# The plugins this application compiles in.\n\
         #\n\
         # Managed by `lat plugin add` and `lat plugin remove`; run `lat plugin sync`\n\
         # and rebuild after editing. Paths are relative to this directory, and folder\n\
         # names carry no meaning: each plugin's own Cargo.toml says what it is called.\n\
         #\n\
         # Enabling and disabling a plugin is separate, and needs no rebuild: it is\n\
         # runtime state, kept in the database and toggled from the admin.\n",
    );
    for entry in entries {
        out.push_str(&format!("\n[[plugin]]\npath = {:?}\n", entry.path));
        if let Some(source) = &entry.source {
            out.push_str(&format!("source = {source:?}\n"));
        }
    }
    let file = list_path(root);
    fs::create_dir_all(file.parent().unwrap())?;
    fs::write(&file, out).with_context(|| format!("writing {}", file.display()))?;
    Ok(())
}

/// Creates the plugin layout for a new application: an empty list and an empty
/// generated manifest, so `lat plugin add` works on a fresh app with no
/// hand-editing. Called by `lat new`.
pub fn scaffold(root: &Path) -> Result<()> {
    write_list(root, &[])?;
    write_manifest(&root.join(MANIFEST_DIR), &[])
}

// ---------------------------------------------------------------- commands

fn add(root: &Path, source: &str, folder: Option<&str>) -> Result<()> {
    let dir = root.join(PLUGINS_DIR);
    fs::create_dir_all(&dir)?;

    let existing: Vec<Entry> = read_list(root)?
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.entry)
        .collect();

    // A local source can be inspected before anything is created, so the common
    // mistake (installing a plugin twice) is caught without leaving debris.
    if !is_git_url(source) {
        if let Ok(name) = read_crate_name(&PathBuf::from(source).join("Cargo.toml")) {
            refuse_duplicate(&dir, &existing, &name)?;
        }
    }

    let (entry, created) = if is_git_url(source) {
        (clone(&dir, source, folder)?, true)
    } else {
        local(&dir, source, folder)?
    };

    // Anything that fails from here removes what this command created, so a
    // refused add leaves the tree exactly as it found it.
    let result = finish_add(root, &dir, &entry, &existing);
    if result.is_err() && created {
        let target = dir.join(&entry.path);
        let _ = if target.is_symlink() {
            fs::remove_file(&target)
        } else {
            fs::remove_dir_all(&target)
        };
    }
    result
}

fn finish_add(root: &Path, dir: &Path, entry: &Entry, existing: &[Entry]) -> Result<()> {
    let manifest = dir.join(&entry.path).join("Cargo.toml");
    if !manifest.is_file() {
        bail!(
            "{} has no Cargo.toml, so it is not a plugin crate",
            dir.join(&entry.path).display()
        );
    }
    let parsed = read_manifest(&manifest)?;
    let crate_name = parsed.package.name.clone();
    if let Some(found) = existing.iter().find(|e| e.path == entry.path) {
        println!("Already installed: {} ({})", crate_name, found.path);
        return Ok(());
    }
    refuse_duplicate(dir, existing, &crate_name)?;
    let module_id = check_compatible(root, &parsed, &crate_name)?;

    let mut entries = existing.to_vec();
    entries.push(entry.clone());
    write_list(root, &entries)?;
    sync(root)?;
    // Announced only now: a link reported before the checks would be a link a
    // refusal then silently removed.
    let target = dir.join(&entry.path);
    if target.is_symlink() {
        let at = fs::read_link(&target).unwrap_or_else(|_| target.clone());
        println!(
            "\nLinked {PLUGINS_DIR}/{} -> {} (it stays where it is; edits apply in place).",
            entry.path,
            at.display()
        );
        println!("Added {module_id} ({crate_name}).");
    } else {
        println!(
            "\nAdded {module_id} ({crate_name}) at {PLUGINS_DIR}/{}.",
            entry.path
        );
    }
    println!("Rebuild to pick it up; enable or disable it from the admin afterwards.");
    Ok(())
}

/// Two folders holding one crate would generate the same dependency key twice
/// and the manifest would not parse. Refused here, where the fix is obvious.
fn refuse_duplicate(dir: &Path, existing: &[Entry], crate_name: &str) -> Result<()> {
    for other in existing {
        let manifest = dir.join(&other.path).join("Cargo.toml");
        if read_crate_name(&manifest).ok().as_deref() == Some(crate_name) {
            bail!(
                "{crate_name} is already installed at {PLUGINS_DIR}/{}. A plugin can \
                 only be compiled in once; remove that one first if you meant to \
                 move it.",
                other.path
            );
        }
    }
    Ok(())
}

fn remove(root: &Path, name: &str, delete: bool) -> Result<()> {
    let plugins = read_list(root)?.unwrap_or_default();
    // Accept the crate name, the folder, or the dotted module id, since those are
    // the three ways a person might refer to the same plugin.
    let dotted = name.replace('.', "-");
    let Some(found) = plugins
        .iter()
        .find(|p| p.crate_name == name || p.entry.path == name || p.crate_name == dotted)
    else {
        bail!("no plugin called '{name}'. `lat plugin list` shows what is installed.");
    };

    let path = found.entry.path.clone();
    let crate_name = found.crate_name.clone();
    let kept: Vec<Entry> = plugins
        .into_iter()
        .filter(|p| p.entry.path != path)
        .map(|p| p.entry)
        .collect();
    write_list(root, &kept)?;

    if delete {
        let target = root.join(PLUGINS_DIR).join(&path);
        // A symlink is removed, never followed: the checkout it points at is
        // somebody's working copy, not ours to delete.
        if target.is_symlink() {
            fs::remove_file(&target)?;
        } else {
            fs::remove_dir_all(&target)
                .with_context(|| format!("removing {}", target.display()))?;
        }
        println!("Deleted {PLUGINS_DIR}/{path}.");
    }
    sync(root)?;
    println!("\nRemoved {crate_name}. Rebuild to drop it from the binary.");
    if !delete {
        println!("Its folder is still at {PLUGINS_DIR}/{path}; `--delete` removes that too.");
    }
    Ok(())
}

/// The module id a crate declares in its plugin marker, if it has one.
pub fn declared_module(crate_dir: &Path) -> Option<String> {
    read_manifest(&crate_dir.join("Cargo.toml"))
        .ok()?
        .package
        .metadata?
        .laterite
        .map(|m| m.plugin)
}

/// Reports where a plugin departs from the expected layout.
///
/// Every plugin has the same shape so that opening an unfamiliar one tells you
/// where things are before you read it. These are the departures that bite:
/// three plugins written by hand ended up with three layouts, and one of them
/// had its migrations in a single file, which works until a second migration
/// arrives.
///
/// Advisory, not fatal. A plugin is somebody else's code, and a layout opinion
/// is not grounds for refusing to build.
fn check(root: &Path) -> Result<()> {
    let Some(plugins) = read_list(root)? else {
        println!("This application has no {PLUGINS_DIR}/ directory.");
        return Ok(());
    };
    let dir = root.join(PLUGINS_DIR);
    let mut total = 0;

    for plugin in &plugins {
        let at = dir.join(&plugin.entry.path);
        let src = at.join("src");
        let mut notes = Vec::new();

        if at.join("src/migrations.rs").is_file() {
            notes.push(
                "migrations are a single file; `migration_set!` expects \
                 src/migrations/ with one file per migration"
                    .to_string(),
            );
        }
        if src.join("admin.rs").is_file() {
            notes.push("admin is a single file; src/admin/ is the shape it grows into".to_string());
        }
        if !src.join("lib.rs").is_file() {
            notes.push(
                "no src/lib.rs, so there is no crate root to expose module() from".to_string(),
            );
        }
        let manifest = at.join("Cargo.toml");
        if read_manifest(&manifest)
            .ok()
            .and_then(|m| m.package.metadata.and_then(|meta| meta.laterite))
            .is_none()
        {
            notes.push(
                "no [package.metadata.laterite] marker naming the module it registers".to_string(),
            );
        }

        if notes.is_empty() {
            println!("  ok    {}", plugin.crate_name);
        } else {
            for note in &notes {
                println!("  warn  {}: {note}", plugin.crate_name);
            }
            total += notes.len();
        }
    }

    if total == 0 {
        println!("\nEvery plugin matches the expected layout.");
    } else {
        println!("\n{total} departure(s) from the expected layout.");
    }
    Ok(())
}

fn sync(root: &Path) -> Result<()> {
    let Some(plugins) = read_list(root)? else {
        bail!(
            "no {PLUGINS_DIR}/ directory here; run this from a Laterite application \
             that uses plugins"
        );
    };
    write_manifest(&root.join(MANIFEST_DIR), &plugins)?;
    if plugins.is_empty() {
        println!("No plugins listed in {PLUGINS_DIR}/{LIST_FILE}; wrote an empty {MANIFEST_DIR}.");
    } else {
        println!("Synced {} plugin(s) into {MANIFEST_DIR}/:", plugins.len());
        for p in &plugins {
            println!("  {:<26} {PLUGINS_DIR}/{}", p.crate_name, p.entry.path);
        }
    }
    warn_if_unwired(root);
    Ok(())
}

fn list(root: &Path) -> Result<()> {
    let Some(plugins) = read_list(root)? else {
        println!("This application has no {PLUGINS_DIR}/ directory.");
        return Ok(());
    };
    if plugins.is_empty() {
        println!("No plugins listed in {PLUGINS_DIR}/{LIST_FILE}.");
        return Ok(());
    }
    for p in &plugins {
        println!("{:<26} {PLUGINS_DIR}/{}", p.crate_name, p.entry.path);
    }
    println!(
        "\nThese are compiled in. Which are enabled is runtime state: see the \
         admin's Plugins screen."
    );
    Ok(())
}

/// A generated manifest nothing depends on is the failure this command used to
/// report as success: `sync` wrote the file, the application never linked it, and
/// the plugin silently did nothing.
fn warn_if_unwired(root: &Path) {
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    if cargo.contains("plugins-manifest") {
        return;
    }
    eprintln!(
        "\nwarning: this application's Cargo.toml does not depend on {MANIFEST_DIR}, so \
         nothing it generates is compiled in.\n\
         \x20        Add `plugins-manifest = {{ path = \"{MANIFEST_DIR}\" }}` to \
         [dependencies], list it under [workspace] members, and register it with\n\
         \x20        `.modules(plugins_manifest::all())` in main.rs."
    );
}

// ---------------------------------------------------------------- sources

fn is_git_url(source: &str) -> bool {
    source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("git@")
        || source.ends_with(".git")
}

/// Clones into `plugins/<name>` and records the URL for provenance.
fn clone(dir: &Path, url: &str, folder: Option<&str>) -> Result<Entry> {
    let name = folder.map(str::to_string).unwrap_or_else(|| {
        url.trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("plugin")
            .trim_end_matches(".git")
            .to_string()
    });
    let target = dir.join(&name);
    if target.exists() {
        bail!(
            "{} already exists; pass --as to choose another folder",
            target.display()
        );
    }
    let status = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(&target)
        .status()
        .context("running git clone (is git installed?)")?;
    if !status.success() {
        bail!("git clone failed for {url}");
    }
    Ok(Entry {
        path: name,
        source: Some(url.to_string()),
    })
}

/// Records a local directory. One inside the application is recorded as a
/// relative path; one outside is symlinked into `plugins/` first, so the list
/// stays portable across machines instead of carrying somebody's home directory.
fn local(dir: &Path, source: &str, folder: Option<&str>) -> Result<(Entry, bool)> {
    let from = PathBuf::from(source);
    let abs = from
        .canonicalize()
        .with_context(|| format!("{source} does not exist"))?;
    if !abs.is_dir() {
        bail!("{source} is not a directory");
    }
    let plugins = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

    if let Ok(inside) = abs.strip_prefix(&plugins) {
        // Already under plugins/: recorded where it is, nothing created.
        return Ok((
            Entry {
                path: inside.to_string_lossy().replace('\\', "/"),
                source: None,
            },
            false,
        ));
    }

    let name = match folder {
        Some(name) => name.to_string(),
        None => read_crate_name(&abs.join("Cargo.toml"))?,
    };
    let link = dir.join(&name);
    if link.exists() || link.is_symlink() {
        bail!(
            "{} already exists; pass --as to choose another folder",
            link.display()
        );
    }
    symlink(&abs, &link)
        .with_context(|| format!("linking {} to {}", link.display(), abs.display()))?;
    Ok((
        Entry {
            path: name,
            source: None,
        },
        true,
    ))
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

// ------------------------------------------------------- generated manifest

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
/// Whether the generated manifest matches the list. `None` when the application
/// does not use plugins at all, so `lat doctor` can stay quiet about it.
pub fn manifest_in_sync(app_root: &Path) -> Result<Option<bool>> {
    let Some(plugins) = read_list(app_root)? else {
        return Ok(None);
    };
    let dir = app_root.join(MANIFEST_DIR);
    let cargo_ok =
        fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default() == manifest_cargo(&plugins);
    let lib_ok =
        fs::read_to_string(dir.join("src/lib.rs")).unwrap_or_default() == manifest_lib(&plugins);
    Ok(Some(cargo_ok && lib_ok))
}

fn read_crate_name(manifest: &Path) -> Result<String> {
    Ok(read_manifest(manifest)?.package.name)
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

    /// A plugin crate at `plugins/<folder>`, whose name deliberately need not
    /// match the folder: that freedom is the point of the list.
    fn write_plugin(root: &Path, folder: &str, crate_name: &str) {
        write_crate(root, folder, crate_name, Some("acme.thing"), None);
    }

    /// A crate with control over its plugin marker and framework requirement.
    fn write_crate(
        root: &Path,
        folder: &str,
        crate_name: &str,
        plugin_id: Option<&str>,
        framework: Option<&str>,
    ) {
        let dir = root.join(PLUGINS_DIR).join(folder);
        fs::create_dir_all(&dir).unwrap();
        let mut toml = format!("[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\n");
        if let Some(id) = plugin_id {
            toml.push_str(&format!(
                "\n[package.metadata.laterite]\nplugin = \"{id}\"\n"
            ));
        }
        if let Some(req) = framework {
            toml.push_str(&format!("\n[dependencies]\nlaterite-core = \"{req}\"\n"));
        }
        fs::write(dir.join("Cargo.toml"), toml).unwrap();
    }

    /// An application manifest declaring the framework version it builds against.
    fn write_app(root: &Path, framework: &str) {
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
                 [dependencies]\nlaterite-admin = \"{framework}\"\n"
            ),
        )
        .unwrap();
    }

    fn manifest_of(root: &Path, folder: &str) -> CargoManifest {
        read_manifest(&root.join(PLUGINS_DIR).join(folder).join("Cargo.toml")).unwrap()
    }

    fn app() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(PLUGINS_DIR)).unwrap();
        dir
    }

    #[test]
    fn the_list_is_the_source_of_truth_not_the_folder_name() {
        let app = app();
        write_plugin(app.path(), "anything-at-all", "rainmill-discovery");
        write_list(
            app.path(),
            &[Entry {
                path: "anything-at-all".into(),
                source: None,
            }],
        )
        .unwrap();

        let plugins = read_list(app.path()).unwrap().unwrap();
        assert_eq!(plugins.len(), 1);
        // The crate name comes from the crate, never from the folder, so the two
        // cannot drift apart and nothing has to be renamed to be installed.
        assert_eq!(plugins[0].crate_name, "rainmill-discovery");
        assert_eq!(plugins[0].entry.path, "anything-at-all");
    }

    #[test]
    fn a_folder_present_but_unlisted_is_not_compiled_in() {
        let app = app();
        write_plugin(app.path(), "listed", "acme-listed");
        write_plugin(app.path(), "dropped-in", "acme-dropped");
        write_list(
            app.path(),
            &[Entry {
                path: "listed".into(),
                source: None,
            }],
        )
        .unwrap();

        let plugins = read_list(app.path()).unwrap().unwrap();
        assert_eq!(plugins.len(), 1, "only the listed plugin counts");
        assert_eq!(plugins[0].crate_name, "acme-listed");
    }

    #[test]
    fn a_listed_path_that_is_not_a_crate_is_reported_by_name() {
        let app = app();
        write_list(
            app.path(),
            &[Entry {
                path: "gone".into(),
                source: None,
            }],
        )
        .unwrap();
        let err = read_list(app.path()).unwrap_err().to_string();
        assert!(err.contains("gone"), "{err}");
        assert!(
            err.contains("lat plugin remove"),
            "the error says how to fix it"
        );
    }

    /// An application from before the list existed keeps working: its folder
    /// layout is adopted once, rather than reading as no plugins at all.
    #[test]
    fn the_old_folder_layout_is_adopted_once() {
        let app = app();
        write_plugin(app.path(), "rainmill/location", "rainmill-location");
        assert!(!list_path(app.path()).is_file());

        let plugins = read_list(app.path()).unwrap().unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].entry.path, "rainmill/location");
        assert!(list_path(app.path()).is_file(), "the list was written");

        // And the second read comes from the file, not another scan.
        assert_eq!(read_list(app.path()).unwrap().unwrap().len(), 1);
    }

    #[test]
    fn an_application_without_plugins_reports_nothing_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_list(dir.path()).unwrap().is_none());
    }

    #[test]
    fn the_list_round_trips_through_its_own_writer() {
        let app = app();
        let entries = vec![
            Entry {
                path: "one".into(),
                source: None,
            },
            Entry {
                path: "two".into(),
                source: Some("https://example.test/two.git".into()),
            },
        ];
        write_list(app.path(), &entries).unwrap();
        let text = fs::read_to_string(list_path(app.path())).unwrap();
        let parsed: PluginList = toml::from_str(&text).unwrap();

        assert_eq!(parsed.plugins.len(), 2);
        assert_eq!(
            parsed.plugins[1].source.as_deref(),
            Some("https://example.test/two.git")
        );
        // The header explains the split that the file exists to express.
        assert!(text.contains("runtime state"), "{text}");
    }

    #[test]
    fn plugins_are_ordered_by_crate_name_so_the_manifest_is_stable() {
        let app = app();
        write_plugin(app.path(), "b", "acme-zebra");
        write_plugin(app.path(), "a", "acme-alpha");
        write_list(
            app.path(),
            &[
                Entry {
                    path: "b".into(),
                    source: None,
                },
                Entry {
                    path: "a".into(),
                    source: None,
                },
            ],
        )
        .unwrap();
        let names: Vec<String> = read_list(app.path())
            .unwrap()
            .unwrap()
            .iter()
            .map(|p| p.crate_name.clone())
            .collect();
        assert_eq!(names, ["acme-alpha", "acme-zebra"]);
    }

    /// Two folders holding one crate would generate the same dependency key
    /// twice, and cargo would fail on a duplicate key rather than on anything
    /// that names the plugin.
    #[test]
    fn the_same_crate_cannot_be_listed_twice() {
        let app = app();
        write_plugin(app.path(), "here", "acme-thing");
        write_plugin(app.path(), "there", "acme-thing");
        write_list(
            app.path(),
            &[
                Entry {
                    path: "here".into(),
                    source: None,
                },
                Entry {
                    path: "there".into(),
                    source: None,
                },
            ],
        )
        .unwrap();
        let err = read_list(app.path()).unwrap_err().to_string();
        assert!(err.contains("acme-thing"), "{err}");
        assert!(err.contains("only be compiled in once"), "{err}");
    }

    /// The marker is what separates a plugin from any other crate. Without the
    /// check, an ordinary crate is accepted and the failure arrives later as a
    /// compile error about a missing `module()`, which names nothing useful.
    #[test]
    fn a_crate_without_the_marker_is_not_a_plugin() {
        let app = app();
        write_crate(app.path(), "rando", "totally-not-a-plugin", None, None);
        let manifest = manifest_of(app.path(), "rando");
        let err = check_compatible(app.path(), &manifest, "totally-not-a-plugin")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a Laterite plugin"), "{err}");
        // And says how to become one.
        assert!(err.contains("[package.metadata.laterite]"), "{err}");
    }

    #[test]
    fn the_marker_names_the_module_the_crate_registers() {
        let app = app();
        write_crate(app.path(), "p", "acme-blog", Some("acme.blog"), None);
        let manifest = manifest_of(app.path(), "p");
        assert_eq!(
            check_compatible(app.path(), &manifest, "acme-blog").unwrap(),
            "acme.blog"
        );
    }

    /// A plugin built for a Laterite this application is not on is refused
    /// before anything is fetched or built, naming both versions.
    #[test]
    fn a_plugin_for_another_laterite_is_refused_upfront() {
        let app = app();
        write_app(app.path(), "0.7");
        write_crate(app.path(), "old", "acme-old", Some("acme.old"), Some("0.5"));
        let manifest = manifest_of(app.path(), "old");
        let err = check_compatible(app.path(), &manifest, "acme-old")
            .unwrap_err()
            .to_string();
        assert!(err.contains("0.5"), "{err}");
        assert!(err.contains("0.7"), "{err}");
    }

    #[test]
    fn a_plugin_for_this_laterite_is_accepted() {
        let app = app();
        write_app(app.path(), "0.5");
        write_crate(app.path(), "ok", "acme-ok", Some("acme.ok"), Some("0.5"));
        let manifest = manifest_of(app.path(), "ok");
        assert!(check_compatible(app.path(), &manifest, "acme-ok").is_ok());
    }

    /// A plugin developed against a local checkout names no version, so there is
    /// nothing to compare and the check stays out of the way.
    #[test]
    fn a_plugin_with_no_version_requirement_is_not_second_guessed() {
        let app = app();
        write_app(app.path(), "0.5");
        write_crate(app.path(), "dev", "acme-dev", Some("acme.dev"), None);
        let manifest = manifest_of(app.path(), "dev");
        assert!(check_compatible(app.path(), &manifest, "acme-dev").is_ok());
    }

    /// Development mode: the application points at a framework checkout, so the
    /// version comes from that crate rather than from a requirement string.
    #[test]
    fn a_path_dependency_is_followed_to_the_frameworks_own_version() {
        let app = app();
        let framework = app.path().join("vendor/laterite/crates/admin");
        fs::create_dir_all(&framework).unwrap();
        fs::write(
            framework.join("Cargo.toml"),
            "[package]\nname = \"laterite-admin\"\nversion = \"0.9.0\"\n",
        )
        .unwrap();
        fs::write(
            app.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\n\
             laterite-admin = { path = \"vendor/laterite/crates/admin\" }\n",
        )
        .unwrap();
        assert_eq!(
            app_framework_version(app.path()),
            Some(semver::Version::new(0, 9, 0))
        );
    }

    #[test]
    fn manifest_lib_lists_each_plugins_entry_point() {
        let plugins = vec![
            Plugin {
                entry: Entry {
                    path: "one".into(),
                    source: None,
                },
                crate_name: "acme-one".into(),
            },
            Plugin {
                entry: Entry {
                    path: "nested/two".into(),
                    source: None,
                },
                crate_name: "acme-two".into(),
            },
        ];
        let lib = manifest_lib(&plugins);
        assert!(lib.contains("acme_one::module(),"));
        assert!(lib.contains("acme_two::module(),"));
        assert!(lib.contains("pub fn all() -> Vec<Box<dyn Module>>"));
    }

    #[test]
    fn manifest_cargo_declares_each_plugin_by_its_listed_path() {
        let plugins = vec![Plugin {
            entry: Entry {
                path: "nested/two".into(),
                source: None,
            },
            crate_name: "acme-two".into(),
        }];
        let cargo = manifest_cargo(&plugins);
        assert!(
            cargo.contains(r#"acme-two = { path = "../plugins/nested/two" }"#),
            "{cargo}"
        );
    }

    #[test]
    fn an_empty_list_generates_an_empty_manifest() {
        assert!(manifest_lib(&[]).contains("vec![]"));
    }
}
