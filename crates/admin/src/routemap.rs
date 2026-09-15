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
