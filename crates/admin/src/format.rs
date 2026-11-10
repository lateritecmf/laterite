//! A field's format: what a value of it looks like, declared once on the field.
//! The form accepts the shapes people type and stores one, and a `link` column
//! on the same field shows it short and linked.
//!
//! ```yaml
//! fields:
//!   instagram_url:
//!     format: { base: "https://www.instagram.com/", prefix: "@", pattern: "[A-Za-z0-9._]{1,30}", lowercase: true }
//! ```

use laterite_core::{t, Text};
use serde::{Deserialize, Serialize};

/// The format of a profile-style link: an address under `base` naming one
/// account. `https://www.instagram.com/acme`, `instagram.com/acme`, `@acme` and
/// `acme` are all `acme`, stored as `{base}acme`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FieldFormat {
    /// The address every value sits under, ending in `/`: `https://social.acme/`.
    pub base: String,
    /// Written before the name where it is shown, and accepted before it where
    /// it is typed: `@`.
    #[serde(default)]
    pub prefix: String,
    /// What a name may be, as a regular expression the whole name must match.
    #[serde(default)]
    pub pattern: Option<String>,
    /// Stores the name lowercased.
    #[serde(default)]
    pub lowercase: bool,
}

impl FieldFormat {
    /// A format for addresses under `base`.
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            prefix: String::new(),
            pattern: None,
            lowercase: false,
        }
    }

    /// Shows `prefix` before the name, and accepts it where the name is typed.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Refuses a name the regular expression `pattern` does not match whole.
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// Stores the name lowercased.
    pub fn lowercase(mut self) -> Self {
        self.lowercase = true;
        self
    }

    /// Why the format cannot be used, for the boot to refuse it.
    pub(crate) fn check(&self) -> Result<(), String> {
        if !(self.base.starts_with("https://") || self.base.starts_with("http://")) {
            return Err(format!(
                "format base `{}` must be an http(s) address",
                self.base
            ));
        }
        if let Some(p) = &self.pattern {
            regex::Regex::new(&format!("^(?:{p})$"))
                .map_err(|e| format!("format pattern `{p}`: {e}"))?;
        }
        Ok(())
    }

    /// The site the format names, for messages: `social.acme`.
    pub fn host(&self) -> &str {
        let rest = self
            .base
            .split_once("://")
            .map_or(self.base.as_str(), |(_, r)| r);
        let host = rest.split('/').next().unwrap_or(rest);
        host.strip_prefix("www.").unwrap_or(host)
    }

    /// The path every address has after the host: `/` or `/in/`.
    fn base_path(&self) -> &str {
        let rest = self
            .base
            .split_once("://")
            .map_or(self.base.as_str(), |(_, r)| r);
        rest.find('/').map_or("/", |i| &rest[i..])
    }

    /// The name inside `value`, from any shape the format accepts; `None` when
    /// `value` is not one of them.
    pub fn name_in(&self, value: &str) -> Option<String> {
        let value = value.trim();
        let lower = value.to_ascii_lowercase();
        let without_scheme = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .map(|_| &value[value.find("://").unwrap_or(0) + 3..]);
        let host = self.host().to_ascii_lowercase();
        let addressed = without_scheme.or_else(|| {
            let l = value.to_ascii_lowercase();
            (l.starts_with(&host) || l.starts_with(&format!("www.{host}"))).then_some(value)
        });
        let name = match addressed {
            Some(rest) => {
                let l = rest.to_ascii_lowercase();
                let after_host = l
                    .strip_prefix("www.")
                    .unwrap_or(&l)
                    .strip_prefix(&host)?
                    .len();
                let path = &rest[rest.len() - after_host..];
                let path = path.split(['?', '#']).next().unwrap_or("");
                let path = path.strip_prefix(self.base_path().trim_end_matches('/'))?;
                path.trim_start_matches('/')
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .to_string()
            }
            None if value.contains('/') || value.contains(' ') => return None,
            None => value.to_string(),
        };
        let name = name.strip_prefix(self.prefix.as_str()).unwrap_or(&name);
        let name = if self.lowercase {
            name.to_lowercase()
        } else {
            name.to_string()
        };
        if name.is_empty() {
            return None;
        }
        if let Some(p) = &self.pattern {
            let re = regex::Regex::new(&format!("^(?:{p})$")).ok()?;
            if !re.is_match(&name) {
                return None;
            }
        }
        Some(name)
    }

    /// `value` in the one shape it is stored in, `{base}{name}`; empty stays
    /// empty. The message says what was expected when `value` is not one.
    pub fn normalize(&self, value: &str) -> Result<String, Text> {
        if value.trim().is_empty() {
            return Ok(String::new());
        }
        match self.name_in(value) {
            Some(name) => Ok(format!("{}{name}", self.base)),
            None => Err(t!(
                "Must be a link on {host}, or the name on its own.",
                host = self.host()
            )),
        }
    }

    /// How a stored value reads: `{prefix}{name}`, or the value as stored when
    /// it is not under `base`.
    pub fn display(&self, value: &str) -> String {
        match self.name_in(value) {
            Some(name)
                if value
                    .trim()
                    .to_ascii_lowercase()
                    .contains(&self.host().to_ascii_lowercase()) =>
            {
                format!("{}{name}", self.prefix)
            }
            _ => value.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn social() -> FieldFormat {
        FieldFormat::new("https://www.social.acme/")
            .prefix("@")
            .pattern("[A-Za-z0-9._]{1,30}")
            .lowercase()
    }

    #[test]
    fn every_shape_people_type_stores_one_address() {
        let f = social();
        for typed in [
            "https://www.social.acme/Acme.Band/?igsh=xyz",
            "http://social.acme/acme.band",
            "social.acme/acme.band/",
            "www.social.acme/acme.band",
            "@acme.band",
            "acme.band",
            "  ACME.BAND ",
        ] {
            assert_eq!(
                f.normalize(typed).unwrap(),
                "https://www.social.acme/acme.band",
                "{typed}"
            );
        }
        assert_eq!(f.normalize("").unwrap(), "");
    }

    #[test]
    fn anything_else_is_refused() {
        let f = social();
        for typed in [
            "https://other.acme/acme",
            "not a name",
            "acme/band",
            "https://www.social.acme/",
            "bad!name",
        ] {
            assert!(f.normalize(typed).is_err(), "{typed}");
        }
    }

    #[test]
    fn a_base_with_a_path_reads_the_segment_after_it() {
        let f = FieldFormat::new("https://www.network.acme/in/");
        assert_eq!(
            f.normalize("network.acme/in/acme-person/").unwrap(),
            "https://www.network.acme/in/acme-person"
        );
        assert!(f
            .normalize("https://www.network.acme/company/acme")
            .is_err());
    }

    #[test]
    fn a_stored_address_reads_as_the_prefixed_name() {
        let f = social();
        assert_eq!(
            f.display("https://www.social.acme/acme.band/"),
            "@acme.band"
        );
        assert_eq!(f.display("something else"), "something else");
    }

    #[test]
    fn the_boot_refuses_a_broken_format() {
        assert!(FieldFormat::new("social.acme/").check().is_err());
        assert!(FieldFormat::new("https://social.acme/")
            .pattern("[")
            .check()
            .is_err());
        assert!(social().check().is_ok());
    }
}
