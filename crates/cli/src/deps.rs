//! The plugins' dependencies against the framework's: what `lat plugin sync`,
//! `lat plugin check` and `lat doctor` report.
//!
//! A crate whose types cross between the framework and a plugin (a pool, a
//! query, a JSON value, a router) must be one version across the build, or the
//! two sides hold types that look alike and do not fit. Cargo builds two
//! versions side by side without complaint, so this reads the resolved graph
//! and names the plugin responsible. Other crates in two versions are harmless
//! and only counted. A clash over a native library stops cargo outright, and its
//! message names packages, so the plugin behind each side is found here too.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Crates whose types cross the plugin boundary, beside every `laterite-*` one.
const BOUNDARY: &[&str] = &[
    "sqlx",
    "sqlx-core",
    "sea-query",
    "serde_json",
    "minijinja",
    "axum",
    "axum-core",
];

/// Crates the framework re-exports, and where a plugin reaches each.
const REEXPORTED: &[(&str, &str)] = &[
    ("sqlx", "laterite_core::sqlx"),
    ("sea-query", "laterite_core::sea_query"),
    ("serde_json", "laterite_core::serde_json"),
    ("minijinja", "laterite_core::minijinja"),
    ("axum", "laterite_admin::axum"),
];

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The application will not build, or will build with split types.
    Error,
    /// Works today; a version change on either side could break it.
    Warning,
    /// For information.
    Note,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// How much it matters.
    pub level: Level,
    /// What to tell the developer.
    pub text: String,
}

impl Finding {
    fn new(level: Level, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
        }
    }
}

/// An installed plugin: its crate name and its folder.
#[derive(Debug, Clone)]
pub struct Installed {
    /// The crate name its manifest declares.
    pub name: String,
    /// The folder its `Cargo.toml` is in.
    pub dir: PathBuf,
}

/// The report for the application at `root`. `verbose` lists the harmless
/// duplicates by name instead of counting them.
pub fn inspect(root: &Path, plugins: &[Installed], verbose: bool) -> Vec<Finding> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .output();
    let out = match out {
        Ok(out) => out,
        Err(e) => {
            return vec![Finding::new(
                Level::Note,
                format!("could not run cargo to read the dependency graph: {e}"),
            )]
        }
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if let Some(finding) = links_conflict(&stderr, plugins) {
            return vec![finding];
        }
        let first = stderr
            .lines()
            .find(|l| l.starts_with("error"))
            .unwrap_or("cargo metadata failed");
        return vec![Finding::new(
            Level::Note,
            format!("could not read the dependency graph: {first}"),
        )];
    }
    match serde_json::from_slice::<Value>(&out.stdout) {
        Ok(meta) => analyze(&meta, plugins, verbose),
        Err(e) => vec![Finding::new(
            Level::Note,
            format!("could not read cargo's dependency graph: {e}"),
        )],
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_boundary(name: &str) -> bool {
    BOUNDARY.contains(&name) || name.starts_with("laterite-") || name == "lateritecmf"
}

/// The findings for a resolved graph (`cargo metadata` output).
pub fn analyze(meta: &Value, plugins: &[Installed], verbose: bool) -> Vec<Finding> {
    let empty = Vec::new();
    let packages = meta["packages"].as_array().unwrap_or(&empty);
    let nodes = meta["resolve"]["nodes"].as_array().unwrap_or(&empty);

    // Each package's id, name and version, and the plugin a package id is.
    let mut by_id: HashMap<&str, (&str, &str)> = HashMap::new();
    let mut plugin_of: HashMap<&str, &str> = HashMap::new();
    let dirs: Vec<(PathBuf, &str)> = plugins
        .iter()
        .map(|p| (canonical(&p.dir), p.name.as_str()))
        .collect();
    for p in packages {
        let (Some(id), Some(name), Some(version)) =
            (p["id"].as_str(), p["name"].as_str(), p["version"].as_str())
        else {
            continue;
        };
        by_id.insert(id, (name, version));
        if let Some(manifest) = p["manifest_path"].as_str() {
            let dir = canonical(Path::new(manifest).parent().unwrap_or(Path::new("")));
            if let Some((_, plugin)) = dirs.iter().find(|(d, n)| *d == dir && *n == name) {
                plugin_of.insert(id, plugin);
            }
        }
    }
    let edges: HashMap<&str, Vec<&str>> = nodes
        .iter()
        .filter_map(|n| {
            let id = n["id"].as_str()?;
            let deps = n["deps"]
                .as_array()
                .map(|d| d.iter().filter_map(|x| x["pkg"].as_str()).collect())
                .unwrap_or_default();
            Some((id, deps))
        })
        .collect();

    // What each plugin pulls in, transitively.
    let mut reach: Vec<(&str, HashSet<&str>)> = Vec::new();
    for (id, plugin) in &plugin_of {
        let mut seen = HashSet::new();
        let mut stack = vec![*id];
        while let Some(at) = stack.pop() {
            if seen.insert(at) {
                if let Some(next) = edges.get(at) {
                    stack.extend(next.iter().copied());
                }
            }
        }
        reach.push((plugin, seen));
    }
    reach.sort_by(|a, b| a.0.cmp(b.0));
    // What the framework pulls in: a version it uses is "the framework's", and
    // the plugins reaching it only through the framework are not named.
    let mut framework: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = by_id
        .iter()
        .filter(|(id, (name, _))| *name == "laterite-core" && edges.contains_key(*id))
        .map(|(id, _)| *id)
        .collect();
    while let Some(at) = stack.pop() {
        if framework.insert(at) {
            if let Some(next) = edges.get(at) {
                stack.extend(next.iter().copied());
            }
        }
    }

    // Every crate name built in more than one version.
    let mut versions: BTreeMap<&str, BTreeSet<(&str, &str)>> = BTreeMap::new();
    for (id, (name, version)) in &by_id {
        if edges.contains_key(id) {
            versions.entry(name).or_default().insert((version, id));
        }
    }
    let mut findings = Vec::new();
    let mut harmless = Vec::new();
    for (name, set) in versions.iter().filter(|(_, s)| s.len() > 1) {
        if !is_boundary(name) {
            harmless.push(*name);
            continue;
        }
        let sides: Vec<String> = set
            .iter()
            .map(|(version, id)| {
                if framework.contains(id) {
                    return format!("{version} for the framework");
                }
                let users: Vec<&str> = reach
                    .iter()
                    .filter(|(_, seen)| seen.contains(id))
                    .map(|(plugin, _)| *plugin)
                    .collect();
                if users.is_empty() {
                    format!("{version} for the framework")
                } else {
                    format!("{version} for {}", users.join(", "))
                }
            })
            .collect();
        findings.push(Finding::new(
            Level::Error,
            format!(
                "{name} is built in {} versions ({}); values of one do not fit the other, \
                 so these cannot exchange them. Move the plugin onto the framework's version.",
                set.len(),
                sides.join("; ")
            ),
        ));
    }

    // A plugin naming a re-exported crate itself.
    for p in packages {
        let Some(plugin) = p["id"].as_str().and_then(|id| plugin_of.get(id)) else {
            continue;
        };
        let deps = p["dependencies"].as_array().unwrap_or(&empty);
        let mut named: Vec<(&str, &str)> = deps
            .iter()
            .filter_map(|d| d["name"].as_str())
            .filter_map(|n| REEXPORTED.iter().find(|(c, _)| *c == n).copied())
            .collect();
        named.sort();
        named.dedup();
        for (krate, path) in named {
            findings.push(Finding::new(
                Level::Warning,
                format!(
                    "{plugin} depends on {krate} itself; reach it through `{path}` so a \
                     version change on either side cannot split its types"
                ),
            ));
        }
    }

    if !harmless.is_empty() {
        let text = if verbose {
            format!(
                "{} other crate(s) are built in more than one version, none crossing a \
                 plugin boundary: {}",
                harmless.len(),
                harmless.join(", ")
            )
        } else {
            format!(
                "{} other crate(s) are built in more than one version, none crossing a \
                 plugin boundary (`lat plugin check` lists them)",
                harmless.len()
            )
        };
        findings.push(Finding::new(Level::Note, text));
    }
    findings
}

/// The packages a cargo dependency chain names, in order: the text between the
/// backticks of each `package `name version (path)``.
fn chain(text: &str) -> Vec<(String, Option<PathBuf>)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(at) = line.rfind("package `") else {
            continue;
        };
        let rest = &line[at + "package `".len()..];
        let Some(end) = rest.find('`') else { continue };
        let inner = &rest[..end];
        let name = inner.split_whitespace().next().unwrap_or("").to_string();
        let path = inner
            .find('(')
            .and_then(|o| inner.rfind(')').map(|c| PathBuf::from(&inner[o + 1..c])));
        out.push((name, path));
    }
    out
}

/// Who a chain belongs to: the first plugin it passes through, else the
/// framework, else the application.
fn owner(chain: &[(String, Option<PathBuf>)], plugins: &[Installed]) -> String {
    let dirs: Vec<(PathBuf, &str)> = plugins
        .iter()
        .map(|p| (canonical(&p.dir), p.name.as_str()))
        .collect();
    for (name, path) in chain {
        if let Some(path) = path {
            let path = canonical(path);
            if let Some((_, plugin)) = dirs.iter().find(|(d, _)| *d == path) {
                return (*plugin).to_string();
            }
        }
        if plugins.iter().any(|p| &p.name == name) {
            return name.clone();
        }
    }
    if chain
        .iter()
        .any(|(n, _)| n.starts_with("laterite-") || n == "lateritecmf")
    {
        "the framework".to_string()
    } else {
        "the application".to_string()
    }
}

/// The finding for cargo's refusal to link one native library twice, or `None`
/// when `stderr` is about something else.
pub fn links_conflict(stderr: &str, plugins: &[Installed]) -> Option<Finding> {
    let marker = "links to the native library `";
    let at = stderr.find(marker)?;
    let rest = &stderr[at + marker.len()..];
    let library = &rest[..rest.find('`')?];
    let split = stderr.find("conflicts with a previous package")?;
    let first = owner(&chain(&stderr[..split]), plugins);
    let second = owner(&chain(&stderr[split..]), plugins);
    let both = if first == second {
        format!("{first} pulls in two packages that")
    } else {
        format!("{first} and {second} each pull in a package that")
    };
    Some(Finding::new(
        Level::Error,
        format!(
            "{both} link the native library `{library}`, and only one may; they cannot be \
             built together. Remove one, or align them on the same version of the crate that \
             links it."
        ),
    ))
}

/// Prints `findings`, one per line, and returns whether any is an error.
pub fn print(findings: &[Finding]) -> bool {
    for f in findings {
        let tag = match f.level {
            Level::Error => "error",
            Level::Warning => "warn ",
            Level::Note => "note ",
        };
        println!("  {tag} {}", f.text);
    }
    findings.iter().any(|f| f.level == Level::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pkg(id: &str, name: &str, version: &str, manifest: &str, deps: &[&str]) -> Value {
        json!({
            "id": id, "name": name, "version": version, "manifest_path": manifest,
            "dependencies": deps.iter().map(|d| json!({ "name": d, "kind": null })).collect::<Vec<_>>(),
        })
    }

    fn installed(name: &str, dir: &str) -> Installed {
        Installed {
            name: name.into(),
            dir: PathBuf::from(dir),
        }
    }

    fn graph() -> Value {
        json!({
            "packages": [
                pkg("app", "acme-app", "0.1.0", "/app/Cargo.toml", &[]),
                pkg("core", "laterite-core", "0.10.0", "/fw/core/Cargo.toml", &["sea-query"]),
                pkg("maps", "acme-maps", "0.1.0", "/app/plugins/acme/maps/Cargo.toml", &["sea-query", "laterite-core"]),
                pkg("blog", "acme-blog", "0.1.0", "/app/plugins/acme/blog/Cargo.toml", &["laterite-core"]),
                pkg("sq32", "sea-query", "0.32.2", "/reg/sea-query-0.32.2/Cargo.toml", &[]),
                pkg("sq31", "sea-query", "0.31.1", "/reg/sea-query-0.31.1/Cargo.toml", &[]),
                pkg("syn2", "syn", "2.0.0", "/reg/syn-2/Cargo.toml", &[]),
                pkg("syn1", "syn", "1.0.0", "/reg/syn-1/Cargo.toml", &[]),
            ],
            "resolve": { "nodes": [
                { "id": "app", "deps": [{ "pkg": "maps" }, { "pkg": "blog" }, { "pkg": "core" }] },
                { "id": "core", "deps": [{ "pkg": "sq32" }, { "pkg": "syn2" }] },
                { "id": "maps", "deps": [{ "pkg": "sq31" }, { "pkg": "core" }, { "pkg": "syn1" }] },
                { "id": "blog", "deps": [{ "pkg": "core" }] },
                { "id": "sq32", "deps": [] },
                { "id": "sq31", "deps": [] },
                { "id": "syn2", "deps": [] },
                { "id": "syn1", "deps": [] },
            ]},
        })
    }

    #[test]
    fn a_split_boundary_crate_names_the_plugin_behind_each_version() {
        let plugins = [
            installed("acme-maps", "/app/plugins/acme/maps"),
            installed("acme-blog", "/app/plugins/acme/blog"),
        ];
        let findings = analyze(&graph(), &plugins, false);
        let error = findings.iter().find(|f| f.level == Level::Error).unwrap();
        assert!(
            error.text.starts_with("sea-query is built in 2 versions"),
            "{}",
            error.text
        );
        assert!(
            error.text.contains("0.31.1 for acme-maps"),
            "{}",
            error.text
        );
        assert!(
            error.text.contains("0.32.2 for the framework"),
            "{}",
            error.text
        );
        let warning = findings.iter().find(|f| f.level == Level::Warning).unwrap();
        assert!(
            warning
                .text
                .contains("acme-maps depends on sea-query itself"),
            "{}",
            warning.text
        );
        assert!(warning.text.contains("laterite_core::sea_query"));
        let note = findings.iter().find(|f| f.level == Level::Note).unwrap();
        assert!(note.text.starts_with("1 other crate(s)"), "{}", note.text);
        assert!(!note.text.contains("syn"), "counted, not listed");
        let verbose = analyze(&graph(), &plugins, true);
        assert!(verbose
            .iter()
            .any(|f| f.level == Level::Note && f.text.ends_with(": syn")));
    }

    #[test]
    fn a_clean_graph_has_no_errors_or_warnings() {
        let mut g = graph();
        g["packages"][2]["dependencies"] = json!([{ "name": "laterite-core", "kind": null }]);
        g["resolve"]["nodes"][2]["deps"] = json!([{ "pkg": "core" }]);
        // Nothing depends on 0.31 any more, so cargo would not resolve it.
        let nodes = g["resolve"]["nodes"].as_array_mut().unwrap();
        nodes.retain(|n| n["id"] != "sq31");
        let plugins = [installed("acme-maps", "/app/plugins/acme/maps")];
        let findings = analyze(&g, &plugins, false);
        assert!(
            findings.iter().all(|f| f.level == Level::Note),
            "{findings:?}"
        );
    }

    #[test]
    fn a_native_library_clash_names_both_sides() {
        let stderr = "\
error: failed to select a version for `libsqlite3-sys`.
    ... required by package `rusqlite v0.29.0`
    ... which satisfies dependency `rusqlite = \"^0.29\"` of package `acme-maps v0.1.0 (/app/plugins/acme/maps)`
    ... which satisfies path dependency `acme-maps` (locked to 0.1.0) of package `plugins-manifest v0.1.0 (/app/plugins-manifest)`
versions that meet the requirements `^0.26.0` are: 0.26.0

the package `libsqlite3-sys` links to the native library `sqlite3`, but it conflicts with a previous package which links to `sqlite3` as well:
package `libsqlite3-sys v0.30.1`
    ... which satisfies dependency `libsqlite3-sys = \"^0.30.1\"` of package `sqlx-sqlite v0.8.6`
    ... which satisfies dependency `sqlx-sqlite = \"=0.8.6\"` of package `sqlx v0.8.6`
    ... which satisfies dependency `sqlx = \"^0.8\"` of package `laterite-core v0.10.0 (/fw/crates/core)`
Only one package in the dependency graph may specify the same links value.
";
        let plugins = [installed("acme-maps", "/app/plugins/acme/maps")];
        let finding = links_conflict(stderr, &plugins).unwrap();
        assert_eq!(finding.level, Level::Error);
        assert!(
            finding
                .text
                .starts_with("acme-maps and the framework each pull in a package"),
            "{}",
            finding.text
        );
        assert!(finding.text.contains("`sqlite3`"));
        assert!(links_conflict("error: could not find `Cargo.toml`", &plugins).is_none());
    }
}
