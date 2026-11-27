//! The public site's content security policy.
//!
//! Every response carries the admin's policy
//! ([`CONTENT_SECURITY_POLICY`](crate::CONTENT_SECURITY_POLICY)). A module
//! whose pages load from another origin (a statistics script, a font host, a
//! payment frame) names that origin with
//! [`allow_site_source`](crate::AdminRegistry::allow_site_source); pages
//! outside the admin then carry the policy with the origin added. The admin's
//! own policy is fixed.

use std::fmt;

/// The part of the policy an origin is allowed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Directive {
    /// Scripts (`script-src`).
    Script,
    /// Stylesheets (`style-src`).
    Style,
    /// Images (`img-src`).
    Image,
    /// Audio and video (`media-src`).
    Media,
    /// Fonts (`font-src`).
    Font,
    /// Requests a page's script makes (`connect-src`).
    Connect,
    /// Pages shown in a frame (`frame-src`).
    Frame,
    /// Where a form may post (`form-action`).
    FormAction,
}

impl Directive {
    /// The directive as the header names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Script => "script-src",
            Self::Style => "style-src",
            Self::Image => "img-src",
            Self::Media => "media-src",
            Self::Font => "font-src",
            Self::Connect => "connect-src",
            Self::Frame => "frame-src",
            Self::FormAction => "form-action",
        }
    }
}

/// An origin the public site may load from, in one part of the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteSource {
    /// The part of the policy the origin is allowed in.
    pub directive: Directive,
    /// The origin: `https://cdn.acme.com`, `https://*.acme.com` or
    /// `wss://live.acme.com`, with an optional port and path.
    pub source: String,
}

impl SiteSource {
    /// Allows `source` in `directive`.
    pub fn new(directive: Directive, source: impl Into<String>) -> Self {
        Self {
            directive,
            source: source.into(),
        }
    }
}

/// A source the policy refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyError {
    /// The source as it was declared.
    pub source: String,
    /// What is wrong with it.
    pub reason: &'static str,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "site source `{}` refused: {}", self.source, self.reason)
    }
}

impl std::error::Error for PolicyError {}

/// Checks one declared origin: a named host over `https` or `wss`, with at
/// most a leading `*.` for its subdomains.
fn check(source: &str) -> Result<(), &'static str> {
    let rest = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("wss://"))
        .ok_or("it must start with https:// or wss://")?;
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if port.is_some_and(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return Err("its port is not a number");
    }
    let named = host.strip_prefix("*.").unwrap_or(host);
    let labels: Vec<&str> = named.split('.').collect();
    let label_ok = |label: &&str| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if labels.len() < 2 || !labels.iter().all(label_ok) {
        return Err("it must name a host, such as cdn.acme.com or *.acme.com");
    }
    let path_ok = path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/-._~%".contains(&b));
    if !path_ok {
        return Err("its path holds a character a policy cannot carry");
    }
    Ok(())
}

/// The site's policy: `base` with each declared origin added to its
/// directive. `None` when nothing was declared, so the site keeps `base`.
pub fn site_policy(base: &str, sources: &[SiteSource]) -> Result<Option<String>, PolicyError> {
    if sources.is_empty() {
        return Ok(None);
    }
    for declared in sources {
        check(&declared.source).map_err(|reason| PolicyError {
            source: declared.source.clone(),
            reason,
        })?;
    }
    let mut parts: Vec<String> = base.split(';').map(|p| p.trim().to_owned()).collect();
    for declared in sources {
        let name = declared.directive.name();
        let part = parts.iter_mut().find(|p| p.split(' ').next() == Some(name));
        match part {
            Some(part) => {
                if !part.split(' ').any(|word| word == declared.source) {
                    part.push(' ');
                    part.push_str(&declared.source);
                }
            }
            None => parts.push(format!("{name} 'self' {}", declared.source)),
        }
    }
    Ok(Some(parts.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "default-src 'self'; script-src 'self'; img-src 'self' data:";

    #[test]
    fn nothing_declared_keeps_the_base() {
        assert_eq!(site_policy(BASE, &[]), Ok(None));
    }

    #[test]
    fn an_origin_joins_its_directive_once() {
        let sources = [
            SiteSource::new(Directive::Script, "https://tags.acme.com"),
            SiteSource::new(Directive::Script, "https://tags.acme.com"),
            SiteSource::new(Directive::Image, "https://*.acme.com"),
        ];
        assert_eq!(
            site_policy(BASE, &sources).unwrap().as_deref(),
            Some(
                "default-src 'self'; script-src 'self' https://tags.acme.com; \
                 img-src 'self' data: https://*.acme.com"
            )
        );
    }

    #[test]
    fn a_directive_the_base_lacks_is_added() {
        let sources = [SiteSource::new(Directive::Font, "https://fonts.acme.com")];
        assert!(site_policy(BASE, &sources)
            .unwrap()
            .unwrap()
            .ends_with("; font-src 'self' https://fonts.acme.com"));
    }

    #[test]
    fn loose_sources_are_refused() {
        for source in [
            "*",
            "https:",
            "https://*",
            "https://*.com",
            "http://cdn.acme.com",
            "'unsafe-inline'",
            "https://cdn.acme.com; script-src *",
            "https://cdn.acme.com 'unsafe-eval'",
            "https://cdn.acme.com:port",
            "https://a.*.acme.com",
        ] {
            let sources = [SiteSource::new(Directive::Script, source)];
            assert!(site_policy(BASE, &sources).is_err(), "{source}");
        }
    }

    #[test]
    fn a_port_and_a_path_are_kept() {
        let sources = [SiteSource::new(
            Directive::Connect,
            "wss://live.acme.com:8443/feed",
        )];
        assert!(site_policy(BASE, &sources).is_ok());
    }
}
