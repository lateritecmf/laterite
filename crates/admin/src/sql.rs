//! Small helpers for building descriptor-driven SQL safely.
//!
//! Identifiers (table and column names) come from developer-authored
//! descriptors, not from request input, but are validated and quoted anyway.
//! Values are always parameterized, never interpolated.

/// Whether a string is a safe, unquoted SQL identifier (lower-snake, <= 63).
pub(crate) fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_lowercase() || (i > 0 && b.is_ascii_digit()))
}

/// Double-quotes an identifier for interpolation into SQL. Only call on strings
/// that have passed [`valid_ident`].
pub(crate) fn quote(ident: &str) -> String {
    format!("\"{ident}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_validation() {
        assert!(valid_ident("backend_users"));
        assert!(valid_ident("created_at"));
        assert!(!valid_ident("Users"));
        assert!(!valid_ident("drop table"));
        assert!(!valid_ident("a-b"));
        assert!(!valid_ident(""));
        assert!(!valid_ident("1col"));
    }
}
