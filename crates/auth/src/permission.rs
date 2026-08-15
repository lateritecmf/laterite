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
//!
//! Role grants form the base. A user may carry per-permission **overrides** that
//! take precedence: an explicit deny wins over everything below a superuser, and
//! an explicit allow wins over the role. A permission with no override inherits
//! the role decision. Overrides are exact codes (no wildcards), so they refine a
//! wildcard role grant one permission at a time.

use std::collections::HashSet;

/// The effective permissions of an identity.
#[derive(Debug, Clone, Default)]
pub struct PermissionSet {
    superuser: bool,
    grants: HashSet<String>,
    allow: HashSet<String>,
    deny: HashSet<String>,
}

impl PermissionSet {
    /// Builds a set from granted permission strings, with no per-user overrides.
    /// A superuser short-circuits every check regardless of the listed grants.
    pub fn new(superuser: bool, grants: impl IntoIterator<Item = String>) -> Self {
        Self {
            superuser,
            grants: grants.into_iter().collect(),
            allow: HashSet::new(),
            deny: HashSet::new(),
        }
    }

    /// Builds a set from role `grants` plus per-user overrides: `allow` forces a
    /// permission on and `deny` forces it off, both taking precedence over the
    /// role grants (deny over allow).
    pub fn with_overrides(
        superuser: bool,
        grants: impl IntoIterator<Item = String>,
        allow: impl IntoIterator<Item = String>,
        deny: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            superuser,
            grants: grants.into_iter().collect(),
            allow: allow.into_iter().collect(),
            deny: deny.into_iter().collect(),
        }
    }

    pub fn is_superuser(&self) -> bool {
        self.superuser
    }

    /// Whether this identity is allowed `needed`. A superuser is always allowed.
    /// Otherwise a user override decides first (deny wins, then allow); with no
    /// override the role grants decide.
    pub fn allows(&self, needed: &str) -> bool {
        if self.superuser {
            return true;
        }
        if self.deny.contains(needed) {
            return false;
        }
        if self.allow.contains(needed) {
            return true;
        }
        self.role_allows(needed)
    }

    /// Whether the role grants (ignoring overrides) allow `needed`.
    fn role_allows(&self, needed: &str) -> bool {
        if self.grants.contains("*") || self.grants.contains(needed) {
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

    #[test]
    fn user_deny_overrides_a_role_grant() {
        // The role grants the whole namespace, but the user is denied one code.
        let p = PermissionSet::with_overrides(
            false,
            ["posts.*".to_string()],
            std::iter::empty(),
            ["posts.approve".to_string()],
        );
        assert!(p.allows("posts.edit"));
        assert!(!p.allows("posts.approve"));
    }

    #[test]
    fn user_allow_grants_beyond_the_role() {
        // The role grants nothing; the user is explicitly allowed one code.
        let p = PermissionSet::with_overrides(
            false,
            std::iter::empty(),
            ["posts.approve".to_string()],
            std::iter::empty(),
        );
        assert!(p.allows("posts.approve"));
        assert!(!p.allows("posts.edit"));
    }

    #[test]
    fn deny_wins_over_allow_and_superuser_ignores_overrides() {
        let denied = PermissionSet::with_overrides(
            false,
            std::iter::empty(),
            ["x.y".to_string()],
            ["x.y".to_string()],
        );
        assert!(!denied.allows("x.y"));
        // A superuser is unaffected by an explicit deny.
        let su = PermissionSet::with_overrides(
            true,
            std::iter::empty(),
            std::iter::empty(),
            ["x.y".to_string()],
        );
        assert!(su.allows("x.y"));
    }
}
