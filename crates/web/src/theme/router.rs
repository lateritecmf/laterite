//! Page URLs as patterns, matched against a request's path and built back
//! from a page and its parameters.
//!
//! A pattern is `/`-separated segments. A static segment matches itself,
//! case-insensitively. A `:name` segment matches one segment and binds it;
//! `:name?` is optional, `:name?value` is optional with a default, `:name|^re$`
//! must match the expression, and `:name*` is a wildcard that takes one or
//! more segments (`:name?*` none or more). An optional segment followed by a
//! required one is required. One wildcard at most, with no expression and
//! nothing optional after it.
//!
//! A [`RouteTable`] holds a pattern per page, ordered so the most specific
//! matches first: more static segments first, then fewer wildcards, then fewer
//! parameters.

use std::collections::BTreeMap;
use std::fmt;

/// The value a required parameter takes when a URL is built without it.
const DEFAULT_VALUE: &str = "default";

/// Why a pattern is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(pub String);

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PatternError {}

#[derive(Debug, Clone)]
enum Segment {
    Static(String),
    Param {
        name: String,
        optional: bool,
        default: Option<String>,
        regex: Option<regex::Regex>,
        wildcard: bool,
    },
}

/// A parsed URL pattern.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    segments: Vec<Segment>,
    static_count: usize,
    dynamic_count: usize,
    wild_count: usize,
}

/// The parameters a match bound, by name; an optional segment the URL left
/// out holds its default, or is absent when it has none.
pub type Params = BTreeMap<String, String>;

fn normalize(path: &str) -> String {
    let mut p = path.to_string();
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    p
}

fn segmentize(path: &str) -> Vec<String> {
    normalize(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl Pattern {
    /// Parses a pattern, refusing one the grammar does not allow.
    pub fn parse(source: &str) -> Result<Pattern, PatternError> {
        if !source.starts_with('/') {
            return Err(PatternError("a URL starts with a slash".into()));
        }
        let mut segments = Vec::new();
        let (mut static_count, mut dynamic_count, mut wild_count) = (0, 0, 0);
        for raw in segmentize(source) {
            if let Some(spec) = raw.strip_prefix(':') {
                let (head, regex) = match spec.split_once('|') {
                    Some((h, r)) => (h.to_string(), Some(r.to_string())),
                    None => (spec.to_string(), None),
                };
                let (name_part, default) = match head.split_once('?') {
                    Some((n, d)) => (n.to_string(), Some(d.to_string())),
                    None => (head.clone(), None),
                };
                let optional = default.is_some();
                let wildcard = name_part.ends_with('*') || default.as_deref() == Some("*");
                let name = name_part.trim_end_matches('*').to_string();
                let default = default
                    .map(|d| d.trim_end_matches('*').to_string())
                    .filter(|d| !d.is_empty());
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(PatternError(format!(
                        "{raw:?} is not a parameter: a name is letters, digits and underscores"
                    )));
                }
                if let Some(d) = &default {
                    if d.contains(['*', '|', '?']) {
                        return Err(PatternError(format!(
                            "the default of {raw:?} cannot contain *, | or ?"
                        )));
                    }
                }
                if wildcard && regex.is_some() {
                    return Err(PatternError(format!(
                        "the wildcard {raw:?} cannot carry an expression"
                    )));
                }
                if wildcard {
                    wild_count += 1;
                    if wild_count > 1 {
                        return Err(PatternError("a URL has at most one wildcard".into()));
                    }
                }
                let regex = match regex {
                    Some(r) if !r.is_empty() => Some(regex::Regex::new(&r).map_err(|e| {
                        PatternError(format!("the expression of {raw:?} does not parse: {e}"))
                    })?),
                    _ => None,
                };
                dynamic_count += 1;
                segments.push(Segment::Param {
                    name,
                    optional,
                    default,
                    regex,
                    wildcard,
                });
            } else {
                if !raw
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                {
                    return Err(PatternError(format!(
                        "{raw:?} is not a URL segment: letters, digits, dots, hyphens and underscores"
                    )));
                }
                static_count += 1;
                segments.push(Segment::Static(raw));
            }
        }
        // Nothing optional may follow the wildcard.
        if let Some(at) = segments
            .iter()
            .position(|s| matches!(s, Segment::Param { wildcard: true, .. }))
        {
            if segments[at + 1..]
                .iter()
                .any(|s| matches!(s, Segment::Param { optional: true, .. }))
            {
                return Err(PatternError(
                    "an optional parameter cannot follow the wildcard".into(),
                ));
            }
        }
        Ok(Pattern {
            source: normalize(source),
            segments,
            static_count,
            dynamic_count,
            wild_count,
        })
    }

    /// The pattern as written, normalized.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether the pattern has parameters.
    pub fn is_static(&self) -> bool {
        self.dynamic_count == 0
    }

    /// The parameter names, in order.
    pub fn params(&self) -> Vec<&str> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                Segment::Param { name, .. } => Some(name.as_str()),
                Segment::Static(_) => None,
            })
            .collect()
    }

    /// An optional segment is required when a required one follows it.
    fn effectively_optional(&self, index: usize) -> bool {
        match &self.segments[index] {
            Segment::Param { optional: true, .. } => self.segments[index + 1..]
                .iter()
                .all(|s| matches!(s, Segment::Param { optional: true, .. })),
            _ => false,
        }
    }

    /// The parameters when `path` matches, else none.
    pub fn matches(&self, path: &str) -> Option<Params> {
        let mut url: Vec<String> = segmentize(path);
        let mut wild_tail: Vec<String> = Vec::new();
        if self.wild_count == 1 && url.len() > self.segments.len() {
            let extra = url.len() - self.segments.len();
            let at = self
                .segments
                .iter()
                .position(|s| matches!(s, Segment::Param { wildcard: true, .. }))?;
            if url.len() > at {
                wild_tail = url.drain(at + 1..at + 1 + extra).collect();
            }
        }
        if url.len() > self.segments.len() {
            return None;
        }
        let mut params = Params::new();
        for (index, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Static(s) => {
                    let got = url.get(index)?;
                    if !s.eq_ignore_ascii_case(got) {
                        return None;
                    }
                }
                Segment::Param {
                    name,
                    default,
                    regex,
                    wildcard,
                    ..
                } => {
                    let optional = self.effectively_optional(index);
                    match url.get(index) {
                        None if optional => {
                            if let Some(d) = default {
                                params.insert(name.clone(), d.clone());
                            }
                        }
                        None => return None,
                        Some(value) => {
                            if let Some(re) = regex {
                                if !re.is_match(value) {
                                    return None;
                                }
                            }
                            let mut value = value.clone();
                            if *wildcard && !wild_tail.is_empty() {
                                value.push('/');
                                value.push_str(&wild_tail.join("/"));
                            }
                            params.insert(name.clone(), value);
                        }
                    }
                }
            }
        }
        Some(params)
    }

    /// A URL from the pattern and `params`: a parameter left out takes its
    /// default, a required one `default`, and the URL stops after the last
    /// segment a value filled.
    pub fn url(&self, params: &Params) -> String {
        let mut out: Vec<String> = Vec::new();
        let mut last_filled = 0;
        for (index, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Static(s) => out.push(s.clone()),
                Segment::Param {
                    name,
                    optional,
                    default,
                    ..
                } => {
                    let supplied = params
                        .get(name)
                        .filter(|v| !v.is_empty() && Some(v.as_str()) != default.as_deref());
                    match supplied {
                        Some(v) => out.push(v.clone()),
                        None if *optional => {
                            out.push(default.clone().unwrap_or_else(|| DEFAULT_VALUE.into()));
                            continue;
                        }
                        None => out.push(DEFAULT_VALUE.into()),
                    }
                }
            }
            last_filled = index;
        }
        out.truncate(last_filled + 1);
        let joined: String = out.iter().map(|s| format!("/{}", s.trim())).collect();
        normalize(&joined)
    }
}

/// Every page's pattern, keyed by the page's name, in matching order.
#[derive(Debug, Clone, Default)]
pub struct RouteTable {
    rules: Vec<(String, Pattern)>,
}

impl RouteTable {
    /// Adds a page's pattern. The table reorders itself.
    pub fn add(&mut self, name: impl Into<String>, pattern: Pattern) {
        self.rules.push((name.into(), pattern));
        self.rules.sort_by(|(_, a), (_, b)| {
            b.static_count
                .cmp(&a.static_count)
                .then(a.wild_count.cmp(&b.wild_count))
                .then(a.dynamic_count.cmp(&b.dynamic_count))
        });
    }

    /// The first page whose pattern matches `path`, with its parameters.
    pub fn resolve(&self, path: &str) -> Option<(&str, Params)> {
        self.rules
            .iter()
            .find_map(|(name, p)| p.matches(path).map(|params| (name.as_str(), params)))
    }

    /// The URL of the page `name` with `params`, or none when no such page.
    pub fn url(&self, name: &str, params: &Params) -> Option<String> {
        self.rules
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.url(params))
    }

    /// The pages whose pattern has no parameter, with their URLs.
    pub fn fixed(&self) -> Vec<(&str, String)> {
        self.rules
            .iter()
            .filter(|(_, p)| p.is_static())
            .map(|(n, p)| (n.as_str(), p.source.clone()))
            .collect()
    }

    /// The pages, in matching order.
    pub fn names(&self) -> Vec<&str> {
        self.rules.iter().map(|(n, _)| n.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Params {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn static_and_required_parameters() {
        let p = Pattern::parse("/blog/post/:post_id").unwrap();
        assert_eq!(
            p.matches("/blog/post/12").unwrap(),
            params(&[("post_id", "12")])
        );
        assert_eq!(
            p.matches("/BLOG/Post/12").unwrap(),
            params(&[("post_id", "12")])
        );
        assert!(p.matches("/blog/post").is_none());
        assert!(p.matches("/blog/post/12/extra").is_none());
        assert_eq!(p.url(&params(&[("post_id", "7")])), "/blog/post/7");
        assert_eq!(p.url(&params(&[])), "/blog/post/default");
    }

    #[test]
    fn optional_with_default_and_a_required_tail() {
        let p = Pattern::parse("/blog/category/:category_id?10").unwrap();
        assert_eq!(
            p.matches("/blog/category").unwrap(),
            params(&[("category_id", "10")])
        );
        assert_eq!(
            p.matches("/blog/category/3").unwrap(),
            params(&[("category_id", "3")])
        );
        assert_eq!(p.url(&params(&[])), "/blog/category");
        assert_eq!(p.url(&params(&[("category_id", "10")])), "/blog/category");
        assert_eq!(p.url(&params(&[("category_id", "4")])), "/blog/category/4");

        let p = Pattern::parse("/blog/:post_id?/comments").unwrap();
        assert!(
            p.matches("/blog/comments").is_none(),
            "required before a required tail"
        );
        assert!(p.matches("/blog/5/comments").is_some());

        let p = Pattern::parse("/blog/:post_id?").unwrap();
        assert_eq!(p.matches("/blog").unwrap(), params(&[]));
        assert_eq!(p.matches("/blog/").unwrap(), params(&[]));
    }

    #[test]
    fn expressions_and_wildcards() {
        let p = Pattern::parse("/blog/:post_id|^[0-9]+$/comments").unwrap();
        assert!(p.matches("/blog/10/comments").is_some());
        assert!(p.matches("/blog/ten/comments").is_none());
        let p = Pattern::parse("/blog/:post_name?|^[a-z0-9\\-]+$").unwrap();
        assert!(p.matches("/blog/my-post").is_some());
        assert!(p.matches("/blog/My Post").is_none());
        assert_eq!(p.matches("/blog").unwrap(), params(&[]));

        let p = Pattern::parse("/color/:color/make/:make*/edit").unwrap();
        assert_eq!(
            p.matches("/color/brown/make/volkswagen/beetle/retro/edit")
                .unwrap(),
            params(&[("color", "brown"), ("make", "volkswagen/beetle/retro")])
        );
        assert_eq!(
            p.matches("/color/brown/make/vw/edit").unwrap(),
            params(&[("color", "brown"), ("make", "vw")])
        );
        let p = Pattern::parse("/blog/:slug?*").unwrap();
        assert_eq!(p.matches("/blog").unwrap(), params(&[]));
        assert_eq!(p.matches("/blog/a/b").unwrap(), params(&[("slug", "a/b")]));
    }

    #[test]
    fn a_bad_pattern_is_refused() {
        assert!(Pattern::parse("blog").is_err());
        assert!(Pattern::parse("/blog/:a*/:b*").is_err());
        assert!(Pattern::parse("/blog/:a*|^x$").is_err());
        assert!(Pattern::parse("/blog/:a*/:b?").is_err());
        assert!(Pattern::parse("/blog/:id?a*b").is_err());
        assert!(Pattern::parse("/blog/:id|(").is_err());
        assert!(Pattern::parse("/blog/:").is_err());
        assert!(Pattern::parse("/bl og").is_err());
        assert!(Pattern::parse("/").is_ok());
    }

    #[test]
    fn the_table_prefers_the_most_specific_rule() {
        let mut t = RouteTable::default();
        t.add("catchall", Pattern::parse("/:slug*").unwrap());
        t.add("post", Pattern::parse("/blog/:slug").unwrap());
        t.add("blog", Pattern::parse("/blog").unwrap());
        t.add("home", Pattern::parse("/").unwrap());
        t.add("archive", Pattern::parse("/blog/archive").unwrap());
        assert_eq!(t.names(), ["archive", "blog", "post", "home", "catchall"]);
        assert_eq!(t.resolve("/blog/archive").unwrap().0, "archive");
        assert_eq!(t.resolve("/blog/hello").unwrap().0, "post");
        assert_eq!(t.resolve("/blog").unwrap().0, "blog");
        assert_eq!(t.resolve("/").unwrap().0, "home");
        assert_eq!(t.resolve("/a/b/c").unwrap().0, "catchall");
        assert_eq!(
            t.url("post", &params(&[("slug", "x")])).as_deref(),
            Some("/blog/x")
        );
        assert!(t.url("missing", &params(&[])).is_none());
        assert_eq!(t.fixed().len(), 3);
    }
}
