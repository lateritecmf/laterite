//! The permission model.
//!
//! Permissions are dotted strings (`"posts.approve"`, `"users.edit"`).
//! Descriptors elsewhere in the framework declare the permission a nav item,
//! form field, or action requires; this type answers whether a given identity
//! holds it. Grants support two wildcard forms:
//!
//! - `"*"` grants everything.
//! - a trailing `".*"` grants a whole namespace, e.g. `"posts.*"` grants
//!   `"posts.approve"` and `"posts.edit"` but not `"posts"` itself.

use std::collections::HashSet;

/// The effective permissions of an identity.
#[derive(Debug, Clone, Default)]
pub struct PermissionSet {
    superuser: bool,
    grants: HashSet<String>,
}

impl PermissionSet {
    /// Builds a set from granted permission strings. A superuser short-circuits
    /// every check regardless of the listed grants.
    pub fn new(superuser: bool, grants: impl IntoIterator<Item = String>) -> Self {
        Self {
            superuser,
            grants: grants.into_iter().collect(),
        }
    }

    pub fn is_superuser(&self) -> bool {
        self.superuser
    }

    /// Whether this set grants `needed`.
    pub fn allows(&self, needed: &str) -> bool {
        if self.superuser || self.grants.contains("*") || self.grants.contains(needed) {
            return true;
        }
        // A trailing-wildcard grant covers any deeper permission under its
        // namespace prefix.
        needed.match_indices('.').any(|(dot, _)| {
            let mut wildcard = String::with_capacity(dot + 2);
            wildcard.push_str(&needed[..=dot]);
            wildcard.push('*');
            self.grants.contains(&wildcard)
        })
    }

    /// The raw grants, for inspection and rendering. Excludes the superuser flag.
    pub fn grants(&self) -> impl Iterator<Item = &str> {
        self.grants.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(grants: &[&str]) -> PermissionSet {
        PermissionSet::new(false, grants.iter().map(|s| s.to_string()))
    }

    #[test]
    fn exact_grant_matches() {
        let p = set(&["posts.approve"]);
        assert!(p.allows("posts.approve"));
        assert!(!p.allows("posts.edit"));
    }

    #[test]
    fn namespace_wildcard_matches_descendants_only() {
        let p = set(&["posts.*"]);
        assert!(p.allows("posts.approve"));
        assert!(p.allows("posts.edit"));
        assert!(p.allows("posts.tags.create"));
        assert!(!p.allows("posts"));
        assert!(!p.allows("users.edit"));
    }

    #[test]
    fn global_wildcard_and_superuser_match_everything() {
        assert!(set(&["*"]).allows("anything.at.all"));
        let su = PermissionSet::new(true, std::iter::empty());
        assert!(su.allows("anything.at.all"));
        assert!(su.is_superuser());
    }

    #[test]
    fn empty_set_grants_nothing() {
        assert!(!set(&[]).allows("posts.approve"));
    }
}
