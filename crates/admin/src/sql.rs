//! Small helpers for building descriptor-driven SQL safely.
//!
//! Identifiers (table and column names) come from developer-authored
//! descriptors, not from request input. They are rendered through `sea-query`
//! (which quotes them per backend) and validated here as defence in depth.
//! Values are always parameterized, never interpolated.

/// Whether a string is a safe, unquoted SQL identifier (lower-snake, <= 63).
pub(crate) fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_lowercase() || (i > 0 && b.is_ascii_digit()))
}

/// Escapes the `LIKE` wildcards (`%`, `_`) and the escape character (`\`) in a
/// user-supplied search term so they match literally. Pair the resulting pattern
/// with `LikeExpr::escape('\\')`, because SQLite has no default escape character
/// and would otherwise treat `\` as an ordinary byte.
pub(crate) fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
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

    #[test]
    fn like_escaping() {
        assert_eq!(like_escape("plain"), "plain");
        // Wildcards and the escape char are backslash-escaped; other text is intact.
        assert_eq!(like_escape("50%_off"), r"50\%\_off");
        assert_eq!(like_escape(r"a\b"), r"a\\b");
        assert_eq!(like_escape("%%%"), r"\%\%\%");
    }
}
