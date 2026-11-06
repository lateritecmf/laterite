//! Resolving where a module's admin screens mount, and refusing a clash.
//!
//! A contributed path passes through three layers, last winning: the module's
//! own namespace, a base the module declares, then a deployment override. The
//! resolved set is checked for duplicates before any route is built, so a plugin
//! cannot silently shadow another or the panel's own screens.

use std::collections::BTreeMap;

use laterite_core::ModuleId;

/// The path a module's contribution mounts at, relative to the admin root.
///
/// Precedence: an override naming the module and this exact path, then one
/// naming the module, then the module's declared base, then its namespace.
pub(crate) fn resolve(
    module: ModuleId,
    declared: &str,
    module_base: Option<&str>,
    overrides: &BTreeMap<String, String>,
) -> String {
    let id = module.as_str();
    if let Some(exact) = overrides.get(&format!("{id}{declared}")) {
        return normalize(exact);
    }
    if let Some(base) = overrides.get(id) {
        return join(base, declared);
    }
    match module_base {
        Some(base) => join(base, declared),
        // `rainmill.location` owns `/rainmill/location/...`, so two modules
        // cannot collide without one of them claiming a shorter path.
        None => join(&format!("/{}", id.replace('.', "/")), declared),
    }
}

fn join(base: &str, declared: &str) -> String {
    let base = normalize(base);
    let base = base.trim_end_matches('/');
    let declared = declared.trim_start_matches('/');
    if declared.is_empty() {
        return if base.is_empty() {
            "/".into()
        } else {
            base.into()
        };
    }
    format!("{base}/{declared}")
}

fn normalize(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// Aborts the boot when two contributions claim one path, naming both so the
/// fix is obvious. Runs over every admin path before any route is mounted.
///
/// `claims` is each resolved path with whoever claims it, framework screens
/// included.
pub(crate) fn check_collisions(claims: &[(String, String)]) {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (path, claimant) in claims {
        if let Some(first) = seen.insert(path, claimant) {
            if first != claimant {
                panic!(
                    "admin path `{path}` is claimed by both `{first}` and \
                     `{claimant}`; set [backend.paths] for one of them to move it"
                );
            }
        }
    }
}

/// Aborts the boot when a resolved path carries a dotted segment.
///
/// A module identity is dotted (`rainmill.discovery`); a URL is not. A dot in a
/// segment is the signature of an identifier that reached the router without
/// being resolved into a path, which is how the settings screens spent months
/// mounted at `/settings/rainmill.discovery.robots`: their storage key was used
/// as their URL because they were collected unowned and never passed through
/// [`resolve`].
///
/// Checking the resolved set rather than each call site means the next
/// contribution type that forgets is caught at boot, whatever it is.
///
/// Admin paths only. A public route is a literal the module chose and may well
/// name a file (`/robots.txt`, `/sitemap.xml`), where a dot is the extension
/// rather than an unresolved identifier.
pub(crate) fn check_segments(claims: &[(String, String)]) {
    for (path, claimant) in claims {
        if let Some(segment) = path.split('/').find(|s| s.contains('.')) {
            panic!(
                "admin path `{path}` from `{claimant}` has a dotted segment                  `{segment}`; a module id is dotted but a URL is not, so resolve                  the path through the module's namespace instead of using its                  identifier or storage key as a URL"
            );
        }
    }
}

/// Aborts the boot when a public route clashes, either with another or with the
/// admin mount. Public paths are literal, so nothing namespaces them apart.
pub(crate) fn check_public(claims: &[(String, String)], admin_path: &str) {
    check_collisions(claims);
    for (path, claimant) in claims {
        if path == admin_path || path.starts_with(&format!("{admin_path}/")) {
            panic!(
                "public route `{path}` from `{claimant}` is inside the admin mount \
                 `{admin_path}`; move it or move the panel with backend.path"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "has a dotted segment")]
    fn a_module_id_used_as_a_url_is_refused() {
        // The exact shape the settings screens shipped with: a storage key
        // concatenated into a path instead of resolved through the namespace.
        check_segments(&[(
            "/settings/rainmill.discovery.robots".to_string(),
            "rainmill.discovery".to_string(),
        )]);
    }

    #[test]
    fn a_resolved_module_path_passes() {
        check_segments(&[(
            "/rainmill/discovery/robots".to_string(),
            "rainmill.discovery".to_string(),
        )]);
    }

    fn overrides(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const M: ModuleId = ModuleId::new("rainmill.location");

    #[test]
    fn a_module_namespaces_its_paths_by_default() {
        assert_eq!(
            resolve(M, "/nodes", None, &BTreeMap::new()),
            "/rainmill/location/nodes"
        );
    }

    #[test]
    fn a_declared_base_replaces_the_namespace() {
        assert_eq!(
            resolve(M, "/nodes", Some("/places"), &BTreeMap::new()),
            "/places/nodes"
        );
    }

    #[test]
    fn a_deployment_override_beats_the_module() {
        let cfg = overrides(&[("rainmill.location", "/geography")]);
        assert_eq!(
            resolve(M, "/nodes", Some("/places"), &cfg),
            "/geography/nodes"
        );
    }

    #[test]
    fn the_more_specific_override_wins() {
        let cfg = overrides(&[
            ("rainmill.location", "/geography"),
            ("rainmill.location/nodes", "/places"),
        ]);
        assert_eq!(resolve(M, "/nodes", None, &cfg), "/places");
    }

    #[test]
    fn a_missing_leading_slash_is_forgiven() {
        let cfg = overrides(&[("rainmill.location", "geography")]);
        assert_eq!(resolve(M, "nodes", None, &cfg), "/geography/nodes");
    }

    #[test]
    #[should_panic(expected = "claimed by both")]
    fn two_modules_claiming_one_path_abort_the_boot() {
        check_collisions(&[
            ("/places".to_string(), "rainmill.location".to_string()),
            ("/places".to_string(), "acme.places".to_string()),
        ]);
    }

    #[test]
    #[should_panic(expected = "claimed by both")]
    fn shadowing_a_framework_screen_aborts_the_boot() {
        check_collisions(&[
            ("/roles".to_string(), "laterite".to_string()),
            ("/roles".to_string(), "acme.roles".to_string()),
        ]);
    }

    #[test]
    #[should_panic(expected = "inside the admin mount")]
    fn a_public_route_may_not_shadow_the_panel() {
        check_public(
            &[("/admin/sneaky".to_string(), "acme.evil".to_string())],
            "/admin",
        );
    }

    #[test]
    #[should_panic(expected = "inside the admin mount")]
    fn a_public_route_may_not_claim_the_mount_itself() {
        check_public(&[("/admin".to_string(), "acme.evil".to_string())], "/admin");
    }

    #[test]
    fn an_ordinary_public_route_passes() {
        check_public(
            &[
                ("/robots.txt".to_string(), "acme.seo".to_string()),
                ("/sitemap.xml".to_string(), "acme.seo".to_string()),
            ],
            "/admin",
        );
    }

    #[test]
    fn distinct_paths_pass() {
        check_collisions(&[
            ("/roles".to_string(), "laterite".to_string()),
            ("/places".to_string(), "rainmill.location".to_string()),
        ]);
    }
}

// ---------------------------------------------------------------- the route map

/// One URL the application answers, for `lat routes` and the Developer
/// section: what a deployment reads before changing a path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RouteRow {
    /// `GET`, `POST`, or both.
    pub methods: Vec<String>,
    /// The mounted path, as a browser hits it.
    pub path: String,
    /// `resource`, `screen`, `settings`, `public`, `page` or `framework`.
    pub kind: String,
    /// The module that owns it, or `the framework`.
    pub owner: String,
    /// The permission gating it; none for a public route.
    pub permission: Option<String>,
}

impl RouteRow {
    pub fn new(
        methods: &[&str],
        path: impl Into<String>,
        kind: &str,
        owner: impl Into<String>,
        permission: Option<&str>,
    ) -> Self {
        Self {
            methods: methods.iter().map(|m| m.to_string()).collect(),
            path: path.into(),
            kind: kind.to_string(),
            owner: owner.into(),
            permission: permission.map(str::to_string),
        }
    }
}

/// Rows a module adds to the map at the time it is printed, for URLs that
/// exist only in data (a theme's pages).
#[laterite_core::strata::async_trait]
pub trait RouteSource: Send + Sync {
    async fn routes(&self, db: &laterite_core::Db) -> Vec<RouteRow>;
}

/// A registered [`RouteSource`].
pub struct RouteSourceReg {
    pub source: std::sync::Arc<dyn RouteSource>,
}

impl RouteSourceReg {
    pub fn new(source: std::sync::Arc<dyn RouteSource>) -> Self {
        Self { source }
    }
}

const FRAMEWORK: &str = "the framework";

/// The framework's own admin routes, kept beside the chain in `router()`
/// that mounts them: a row here is a route there. An integration test
/// requests every fixed GET row as a superuser and refuses a 404.
pub fn framework_routes(admin_path: &str) -> Vec<RouteRow> {
    let manage = |permission: &str| Some(permission.to_string());
    let rows: Vec<(&[&str], &str, Option<String>)> = vec![
        (&["GET"], "", None),
        (&["GET", "POST"], "/login", None),
        (&["POST"], "/logout", None),
        (&["GET", "POST"], "/forgot", None),
        (&["GET", "POST"], "/set-password/{token}", None),
        (&["GET", "POST"], "/setup", None),
        (&["GET"], "/settings", None),
        (&["GET", "POST"], "/preferences", None),
        (&["POST"], "/preferences/password", None),
        (&["POST"], "/preferences/menu", None),
        (&["POST"], "/preferences/menu/moves", None),
        (&["POST"], "/preferences/sessions/revoke", None),
        (&["POST"], "/markdown/preview", None),
        (&["GET"], "/editor/menu", None),
        (&["GET"], "/dashboard/widget/{code}", None),
        (&["POST"], "/dashboard/layout", None),
        (&["POST"], "/dashboard/layout/reset", None),
        (&["GET"], "/palette", None),
        (
            &["GET", "POST"],
            "/roles/new",
            manage("backend.manage_roles"),
        ),
        (
            &["GET", "POST"],
            "/roles/{id}/edit",
            manage("backend.manage_roles"),
        ),
        (
            &["GET", "POST"],
            "/users/new",
            manage("backend.manage_users"),
        ),
        (
            &["GET", "POST"],
            "/users/{id}/edit",
            manage("backend.manage_users"),
        ),
        (
            &["POST"],
            "/users/{id}/active",
            manage("backend.manage_users"),
        ),
        (
            &["POST"],
            "/users/{id}/password",
            manage("backend.manage_users"),
        ),
        (
            &["POST"],
            "/users/{id}/unlock",
            manage("backend.manage_users"),
        ),
        (&["GET"], "/plugins", manage("backend.manage_plugins")),
        (
            &["POST"],
            "/plugins/toggle",
            manage("backend.manage_plugins"),
        ),
        (&["GET"], "/jobs", manage("backend.manage_jobs")),
        (&["POST"], "/jobs/retry-all", manage("backend.manage_jobs")),
        (&["POST"], "/jobs/forget-all", manage("backend.manage_jobs")),
        (&["POST"], "/jobs/{id}/retry", manage("backend.manage_jobs")),
        (
            &["POST"],
            "/jobs/{id}/forget",
            manage("backend.manage_jobs"),
        ),
        (&["GET"], "/mail-log", manage("backend.manage_mail")),
        (&["POST"], "/mail-log/clear", manage("backend.manage_mail")),
        (&["GET"], "/mail-log/{id}", manage("backend.manage_mail")),
        (
            &["GET"],
            "/mail-log/{id}/html",
            manage("backend.manage_mail"),
        ),
        (&["GET"], "/mail-templates", manage("backend.manage_mail")),
        (
            &["POST"],
            "/mail-templates/brand-preview",
            manage("backend.manage_mail"),
        ),
        (
            &["GET", "POST"],
            "/mail-templates/{kind}/new",
            manage("backend.manage_mail"),
        ),
        (
            &["GET", "POST"],
            "/mail-templates/{kind}/{code}",
            manage("backend.manage_mail"),
        ),
        (
            &["GET"],
            "/mail-templates/{kind}/{code}/preview",
            manage("backend.manage_mail"),
        ),
        (
            &["POST"],
            "/mail-templates/{kind}/{code}/test",
            manage("backend.manage_mail"),
        ),
        (
            &["POST"],
            "/mail-templates/{kind}/{code}/reset",
            manage("backend.manage_mail"),
        ),
        (
            &["POST"],
            "/mail-templates/{kind}/{code}/delete",
            manage("backend.manage_mail"),
        ),
        (&["GET"], "/scheduled-tasks", manage("backend.manage_jobs")),
        (
            &["POST"],
            "/scheduled-tasks/{name}/run",
            manage("backend.manage_jobs"),
        ),
    ];
    rows.into_iter()
        .map(|(methods, path, permission)| {
            RouteRow::new(
                methods,
                format!("{admin_path}{path}"),
                "framework",
                FRAMEWORK,
                permission.as_deref(),
            )
        })
        .collect()
}

/// A resource's routes, from what its descriptor enables: the same
/// conditions `mount_resource` applies.
pub fn resource_routes(resource: &crate::Resource, admin_path: &str, owner: &str) -> Vec<RouteRow> {
    let base = format!("{admin_path}{}", resource.base_path);
    let permission = resource.permission.as_deref();
    let row = |methods: &[&str], suffix: &str| {
        RouteRow::new(
            methods,
            format!("{base}{suffix}"),
            "resource",
            owner,
            permission,
        )
    };
    let mut rows = vec![row(&["GET"], ""), row(&["POST"], "/columns")];
    if resource.list.columns.iter().any(|c| c.editable) {
        rows.push(row(&["GET", "POST"], "/{id}/cell"));
    }
    if resource.list.exportable {
        rows.push(row(&["GET", "POST"], "/export"));
    }
    if resource.list.deletable {
        rows.push(row(&["POST"], "/delete"));
    }
    if resource
        .list
        .tree
        .as_ref()
        .is_some_and(|t| t.source.is_some())
    {
        rows.push(row(&["GET", "POST"], "/{id}/move"));
    }
    if resource.list.reorder.is_some() {
        rows.push(row(&["POST"], "/reorder"));
    }
    if resource.form.is_some() {
        rows.push(row(&["GET", "POST"], "/new"));
        rows.push(row(&["GET"], "/{id}/reveal/{field}"));
        rows.push(row(&["GET", "POST"], "/{id}/edit"));
        for rel in &resource.relations {
            let name = &rel.name;
            let rel_permission = rel.permission.as_deref().or(permission);
            let rel_row = |methods: &[&str], suffix: &str| {
                RouteRow::new(
                    methods,
                    format!("{base}/{{id}}/{name}{suffix}"),
                    "resource",
                    owner,
                    rel_permission,
                )
            };
            rows.push(rel_row(&["GET", "POST"], ""));
            rows.push(rel_row(&["GET", "POST"], "/new"));
            rows.push(rel_row(&["GET", "POST"], "/{child}/edit"));
            rows.push(rel_row(&["POST"], "/delete"));
            if rel.list.reorder.is_some() {
                rows.push(rel_row(&["POST"], "/reorder"));
            }
        }
    }
    if resource.import.is_some() {
        rows.push(row(&["GET", "POST"], "/import"));
    }
    rows
}

/// The map as a table: methods, path, kind, owner, permission.
pub fn render_table(rows: &[RouteRow]) -> String {
    let cells: Vec<[String; 5]> = rows
        .iter()
        .map(|r| {
            [
                r.methods.join(","),
                r.path.clone(),
                r.kind.clone(),
                r.owner.clone(),
                r.permission.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let header = ["METHOD", "PATH", "KIND", "OWNER", "PERMISSION"];
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let line = |cols: [&str; 5]| -> String {
        cols.iter()
            .enumerate()
            .map(|(i, c)| format!("{:<width$}", c, width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = line(header);
    out.push('\n');
    for row in &cells {
        out.push_str(&line([&row[0], &row[1], &row[2], &row[3], &row[4]]));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod route_map_tests {
    use super::*;
    use crate::form::{FormConfig, FormField};
    use crate::list::{ListColumn, ListConfig};

    #[test]
    fn a_resource_lists_what_its_descriptor_enables() {
        let plain = crate::Resource::new(
            "/notes",
            "Notes",
            ListConfig::new("notes", "Notes", vec![ListColumn::new("title", "Title")]),
        );
        let rows = resource_routes(&plain, "/admin", "acme.notes");
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["/admin/notes", "/admin/notes/columns"]);

        let full = crate::Resource::new(
            "/notes",
            "Notes",
            ListConfig::new("notes", "Notes", vec![ListColumn::new("title", "Title")]).deletable(),
        )
        .form(FormConfig::new(
            "notes",
            "Note",
            "/notes",
            "id",
            vec![FormField::text("title", "Title")],
        ))
        .permission("acme.manage_notes")
        .import(crate::ImportConfig::new());
        let rows = resource_routes(&full, "/admin", "acme.notes");
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert!(paths.contains(&"/admin/notes/new"), "{paths:?}");
        assert!(paths.contains(&"/admin/notes/{id}/edit"), "{paths:?}");
        assert!(paths.contains(&"/admin/notes/delete"), "{paths:?}");
        assert!(paths.contains(&"/admin/notes/import"), "{paths:?}");
        assert!(rows
            .iter()
            .all(|r| r.permission.as_deref() == Some("acme.manage_notes")));
    }

    #[test]
    fn the_table_aligns_its_columns() {
        let rows = vec![
            RouteRow::new(&["GET"], "/admin", "framework", FRAMEWORK, None),
            RouteRow::new(
                &["GET", "POST"],
                "/admin/users/new",
                "framework",
                FRAMEWORK,
                Some("backend.manage_users"),
            ),
        ];
        let table = render_table(&rows);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("METHOD    PATH"), "{}", lines[0]);
        assert!(
            lines[2].starts_with("GET,POST  /admin/users/new"),
            "{}",
            lines[2]
        );
        assert!(lines[2].ends_with("backend.manage_users"), "{}", lines[2]);
    }
}
