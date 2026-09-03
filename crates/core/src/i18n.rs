//! UI-string localization.
//!
//! String-key and deferred: a [`Text`] holds an English source (the lookup key),
//! optional context, plural, and arguments, but no locale. A per-request
//! [`Translator`] localizes it to a `String` at the sink, so a message carries no
//! ambient locale and can cross a redirect or be composed in core.
//!
//! [`Text`] has no `Display`: a bare user-facing string cannot render directly, so
//! the type enforces that every string goes through the translator. Translating
//! stored content is separate (the `translatable` field flag), not this.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A CLDR plural category. The launch locales use only `One`/`Other`; the enum is
/// `non_exhaustive` so the full CLDR set (`Zero`/`Two`/`Few`/`Many`) can arrive with
/// an `icu_plurals` feature without breaking match arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluralCategory {
    One,
    Other,
}

/// The CLDR plural category for `n` in `locale`. A small table for the launch
/// locales; the long tail comes later behind an `icu_plurals` feature. Values are
/// integers, so CLDR `v` is always 0.
pub fn plural_category(locale: &str, n: i64) -> PluralCategory {
    let lang = locale.split(['-', '_']).next().unwrap_or(locale);
    let n = n.unsigned_abs();
    let one = match lang {
        // hi, kn: 0 and 1 are singular (CLDR `i = 0 or n = 1`).
        "hi" | "kn" => n == 0 || n == 1,
        // en, ta, and the default: singular only at 1.
        _ => n == 1,
    };
    if one {
        PluralCategory::One
    } else {
        PluralCategory::Other
    }
}

/// A substitution argument for a `{name}` placeholder in a [`Text`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Arg {
    Str(String),
    Int(i64),
    Num(f64),
    /// A translatable argument, localized recursively at the sink.
    Text(Box<Text>),
}

impl From<String> for Arg {
    fn from(s: String) -> Self {
        Arg::Str(s)
    }
}
impl From<&str> for Arg {
    fn from(s: &str) -> Self {
        Arg::Str(s.to_string())
    }
}
impl From<i64> for Arg {
    fn from(n: i64) -> Self {
        Arg::Int(n)
    }
}
impl From<f64> for Arg {
    fn from(n: f64) -> Self {
        Arg::Num(n)
    }
}
impl From<Text> for Arg {
    fn from(t: Text) -> Self {
        Arg::Text(Box::new(t))
    }
}

/// A locale-free, translatable message; the English source is the lookup key. Build
/// with the `t!`/`tn!`/`tp!` macros or the builders here. Serializes as a plain
/// string when it has no context/plural/args (a descriptor label stays plain on
/// disk), else as a full object (a flash message keeps its args across a round-trip).
/// No `Display`.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    source: Cow<'static, str>,
    context: Option<Cow<'static, str>>,
    plural: Option<(Cow<'static, str>, i64)>,
    args: Vec<(Cow<'static, str>, Arg)>,
}

impl Text {
    /// A message from a static source string (the macro path).
    pub fn new(source: impl Into<Cow<'static, str>>) -> Self {
        Self {
            source: source.into(),
            context: None,
            plural: None,
            args: Vec::new(),
        }
    }

    /// A message from a runtime (non-literal) string: the escape hatch extraction
    /// ignores.
    pub fn dynamic(s: impl Into<String>) -> Self {
        Self::new(Cow::Owned(s.into()))
    }

    /// Disambiguates a homonym source (`Open` the verb vs the adjective). The
    /// context is part of the lookup key, never shown.
    pub fn with_context(mut self, context: impl Into<Cow<'static, str>>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Marks a plural message: `other` is the plural source form, `n` the count the
    /// category is chosen from.
    pub fn with_plural(mut self, other: impl Into<Cow<'static, str>>, n: i64) -> Self {
        self.plural = Some((other.into(), n));
        self
    }

    /// Binds a `{name}` placeholder value.
    pub fn arg(mut self, name: impl Into<Cow<'static, str>>, value: impl Into<Arg>) -> Self {
        self.args.push((name.into(), value.into()));
        self
    }

    /// The source (English) string, the lookup key.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether this is a bare source with no context, plural, or arguments (the form
    /// that serializes as a plain string).
    fn is_plain(&self) -> bool {
        self.context.is_none() && self.plural.is_none() && self.args.is_empty()
    }
}

impl Serialize for Text {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        if self.is_plain() {
            // A plain string on disk, tagged so a collecting serializer can spot it.
            ser.serialize_newtype_struct("laterite.Text", &*self.source)
        } else {
            let mut st = ser.serialize_struct("laterite.Text", 4)?;
            st.serialize_field("source", &*self.source)?;
            st.serialize_field("context", &self.context)?;
            st.serialize_field("plural", &self.plural)?;
            st.serialize_field("args", &self.args)?;
            st.end()
        }
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct TextVisitor;

        impl<'de> Visitor<'de> for TextVisitor {
            type Value = Text;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a source string or a Text object")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Text, E> {
                Ok(Text::dynamic(v))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Text, E> {
                Ok(Text::dynamic(v))
            }

            fn visit_newtype_struct<D: Deserializer<'de>>(self, de: D) -> Result<Text, D::Error> {
                Ok(Text::dynamic(String::deserialize(de)?))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Text, A::Error> {
                let mut source: Option<String> = None;
                let mut context: Option<String> = None;
                let mut plural: Option<(String, i64)> = None;
                let mut args: Vec<(String, Arg)> = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "source" => source = Some(map.next_value()?),
                        "context" => context = map.next_value()?,
                        "plural" => plural = map.next_value()?,
                        "args" => args = map.next_value()?,
                        _ => {
                            let _: de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let source = source.ok_or_else(|| de::Error::missing_field("source"))?;
                Ok(Text {
                    source: Cow::Owned(source),
                    context: context.map(Cow::Owned),
                    plural: plural.map(|(s, n)| (Cow::Owned(s), n)),
                    args: args.into_iter().map(|(k, v)| (Cow::Owned(k), v)).collect(),
                })
            }
        }

        de.deserialize_any(TextVisitor)
    }
}

/// A stored translation for one key in one locale.
#[derive(Debug, Clone)]
enum Entry {
    /// A single form.
    One(String),
    /// Plural forms by category (only `one`/`other` for the launch locales).
    Plural { one: String, other: String },
}

/// Per-locale message catalogs, keyed by source (and context), built from module
/// contributions. File loading arrives with the i18n tooling.
#[derive(Debug, Default)]
pub struct CatalogStore {
    // locale -> (key -> entry). The key is the source, or "context\u{4}source".
    locales: HashMap<String, HashMap<String, Entry>>,
}

impl CatalogStore {
    /// Starts an in-memory catalog; a file loader lands with the tooling.
    pub fn builder() -> CatalogBuilder {
        CatalogBuilder {
            store: CatalogStore::default(),
        }
    }

    fn lookup(&self, locale: &str, key: &str) -> Option<&Entry> {
        self.locales.get(locale).and_then(|m| m.get(key))
    }
}

/// The composite lookup key: the source, prefixed by the context when present.
fn catalog_key(source: &str, context: Option<&str>) -> String {
    match context {
        Some(c) => format!("{c}\u{4}{source}"),
        None => source.to_string(),
    }
}

/// Builds a [`CatalogStore`] from message contributions.
pub struct CatalogBuilder {
    store: CatalogStore,
}

impl CatalogBuilder {
    /// Adds a single-form translation for a locale.
    pub fn message(mut self, locale: &str, source: &str, translation: &str) -> Self {
        self.insert(
            locale,
            catalog_key(source, None),
            Entry::One(translation.to_string()),
        );
        self
    }

    /// Adds a context-disambiguated translation for a locale.
    pub fn message_ctx(
        mut self,
        locale: &str,
        context: &str,
        source: &str,
        translation: &str,
    ) -> Self {
        self.insert(
            locale,
            catalog_key(source, Some(context)),
            Entry::One(translation.to_string()),
        );
        self
    }

    /// Adds plural forms for a locale.
    pub fn plural(mut self, locale: &str, source: &str, one: &str, other: &str) -> Self {
        self.insert(
            locale,
            catalog_key(source, None),
            Entry::Plural {
                one: one.to_string(),
                other: other.to_string(),
            },
        );
        self
    }

    fn insert(&mut self, locale: &str, key: String, entry: Entry) {
        self.store
            .locales
            .entry(locale.to_string())
            .or_default()
            .insert(key, entry);
    }

    pub fn build(self) -> CatalogStore {
        self.store
    }
}

/// Resolves [`Text`] for a request against a shared [`CatalogStore`] over a fallback
/// chain (most specific first). Cheap to clone.
#[derive(Debug, Clone)]
pub struct Translator {
    chain: Arc<[String]>,
    store: Arc<CatalogStore>,
}

impl Translator {
    /// A translator for one locale with no catalogs: every lookup falls back to the
    /// source. The boot-time fallback translator.
    pub fn new(locale: impl Into<String>) -> Self {
        Self {
            chain: Arc::from(vec![locale.into()]),
            store: Arc::new(CatalogStore::default()),
        }
    }

    /// A translator over an explicit fallback chain and a shared store.
    pub fn with_chain(chain: Vec<String>, store: Arc<CatalogStore>) -> Self {
        Self {
            chain: Arc::from(chain),
            store,
        }
    }

    /// The active (most specific) locale.
    pub fn locale(&self) -> &str {
        self.chain.first().map(String::as_str).unwrap_or("en")
    }

    /// Localizes `text`: resolves the form along the chain (else the source), then
    /// interpolates its `{name}` arguments.
    pub fn t(&self, text: &Text) -> String {
        let key = catalog_key(&text.source, text.context.as_deref());
        let template = self.resolve(&key, text);
        interpolate(&template, &text.args, self)
    }

    /// The chosen form string: the first chain locale with an entry (picking the
    /// plural form by that locale's rules), or the English source form.
    fn resolve(&self, key: &str, text: &Text) -> String {
        for locale in self.chain.iter() {
            if let Some(entry) = self.store.lookup(locale, key) {
                return match entry {
                    Entry::One(s) => s.clone(),
                    Entry::Plural { one, other } => {
                        let n = text.plural.as_ref().map(|(_, n)| *n).unwrap_or(1);
                        match plural_category(locale, n) {
                            PluralCategory::One => one.clone(),
                            _ => other.clone(),
                        }
                    }
                };
            }
        }
        // No catalog entry: the English source forms.
        match &text.plural {
            Some((other, n)) => match plural_category("en", *n) {
                PluralCategory::One => text.source.to_string(),
                _ => other.to_string(),
            },
            None => text.source.to_string(),
        }
    }
}

/// Replaces each `{name}` in `template` with its argument's localized value. A
/// translatable argument is localized recursively through `tr`.
fn interpolate(template: &str, args: &[(Cow<'static, str>, Arg)], tr: &Translator) -> String {
    if args.is_empty() {
        return template.to_string();
    }
    let mut out = template.to_string();
    for (name, arg) in args {
        let value = match arg {
            Arg::Str(s) => s.clone(),
            Arg::Int(n) => n.to_string(),
            Arg::Num(n) => n.to_string(),
            Arg::Text(t) => tr.t(t),
        };
        out = out.replace(&format!("{{{name}}}"), &value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<CatalogStore> {
        Arc::new(
            CatalogStore::builder()
                .message(
                    "kn",
                    "Dashboard",
                    "\u{ca1}\u{ccd}\u{caf}\u{cbe}\u{cb6}\u{ccd}\u{cac}\u{ccb}\u{cb0}\u{ccd}\u{ca1}",
                )
                .message(
                    "kn",
                    "Welcome, {name}",
                    "\u{cb8}\u{ccd}\u{cb5}\u{cbe}\u{c97}\u{ca4}, {name}",
                )
                .message_ctx("kn", "verb", "Open", "\u{ca4}\u{cc6}\u{cb0}\u{cc6}")
                .plural(
                    "kn",
                    "{n} item",
                    "{n} \u{c90}\u{c9f}\u{c82}",
                    "{n} \u{c90}\u{c9f}\u{c82}\u{c97}\u{cb3}\u{cc1}",
                )
                .plural("en", "{n} item", "{n} item", "{n} items")
                .build(),
        )
    }

    fn kn() -> Translator {
        Translator::with_chain(vec!["kn".into(), "en".into()], store())
    }

    #[test]
    fn falls_back_to_source_with_no_catalog() {
        let t = Translator::new("en");
        assert_eq!(t.t(&Text::new("Settings")), "Settings");
    }

    #[test]
    fn translates_and_interpolates() {
        let t = kn();
        assert_eq!(
            t.t(&Text::new("Welcome, {name}").arg("name", "Asha")),
            "\u{cb8}\u{ccd}\u{cb5}\u{cbe}\u{c97}\u{ca4}, Asha"
        );
        // A source the catalog does not cover falls back, still interpolating.
        assert_eq!(t.t(&Text::new("Hi {name}").arg("name", "Asha")), "Hi Asha");
    }

    #[test]
    fn context_disambiguates() {
        let t = kn();
        // The `verb` context has an entry; the same source without context falls back.
        assert_eq!(
            t.t(&Text::new("Open").with_context("verb")),
            "\u{ca4}\u{cc6}\u{cb0}\u{cc6}"
        );
        assert_eq!(t.t(&Text::new("Open")), "Open");
    }

    #[test]
    fn plural_picks_the_form_by_locale_rules() {
        let t = kn();
        let msg = |n: i64| {
            t.t(&Text::new("{n} item")
                .with_plural("{n} items", n)
                .arg("n", n))
        };
        // Kannada: 0 and 1 are singular, 2+ plural.
        assert_eq!(msg(0), "0 \u{c90}\u{c9f}\u{c82}");
        assert_eq!(msg(1), "1 \u{c90}\u{c9f}\u{c82}");
        assert_eq!(msg(5), "5 \u{c90}\u{c9f}\u{c82}\u{c97}\u{cb3}\u{cc1}");
    }

    #[test]
    fn plural_source_fallback_uses_english_rules() {
        // No kn/en override for this source: English source forms, English rules.
        let t = kn();
        let msg = |n: i64| {
            t.t(&Text::new("{n} file")
                .with_plural("{n} files", n)
                .arg("n", n))
        };
        assert_eq!(msg(1), "1 file");
        assert_eq!(msg(0), "0 files"); // English: only 1 is singular.
        assert_eq!(msg(3), "3 files");
    }

    #[test]
    fn nested_translatable_argument() {
        // An argument that is itself a Text is localized recursively.
        let t = kn();
        let inner = Text::new("Dashboard");
        let out = t.t(&Text::new("Go to {page}").arg("page", inner));
        assert_eq!(
            out,
            "Go to \u{ca1}\u{ccd}\u{caf}\u{cbe}\u{cb6}\u{ccd}\u{cac}\u{ccb}\u{cb0}\u{ccd}\u{ca1}"
        );
    }

    #[test]
    fn plain_text_serializes_as_a_bare_string() {
        // A descriptor label round-trips as a plain string.
        let json = serde_json::to_string(&Text::new("Name")).unwrap();
        assert_eq!(json, "\"Name\"");
        let back: Text = serde_json::from_str("\"Name\"").unwrap();
        assert_eq!(back, Text::dynamic("Name"));
    }

    #[test]
    fn rich_text_round_trips_as_an_object() {
        // A flash message keeps its arguments across a (session) round-trip.
        let msg = Text::new("Role {name} created").arg("name", "editor");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Text = serde_json::from_str(&json).unwrap();
        assert_eq!(back.source(), "Role {name} created");
        assert_eq!(Translator::new("en").t(&back), "Role editor created");
    }
}
