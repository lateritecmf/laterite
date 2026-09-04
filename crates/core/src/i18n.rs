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
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The QA pseudo-locale: selecting it renders every localized string accented and
/// bracketed, so any unwrapped (non-localized) UI string stands out and clipped
/// layouts show up under the wider text. It needs no catalog.
pub const PSEUDO_LOCALE: &str = "xx";

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

// A descriptor label is authored as a plain string and localized at render: these
// let a builder (`FormField::text("name", "Name")`) and a `.into()` on a descriptor
// literal produce a `Text` without ceremony. The message sinks still take `Text` by
// value, so a bare literal there is a compile error, not a silent conversion.
impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Text::dynamic(s)
    }
}
impl From<String> for Text {
    fn from(s: String) -> Self {
        Text::dynamic(s)
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

    /// The locales this store holds a catalog for, sorted. English is the source
    /// and needs no catalog, so it is never listed; callers add it themselves.
    pub fn locales(&self) -> Vec<String> {
        let mut out: Vec<String> = self.locales.keys().cloned().collect();
        out.sort();
        out
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

    /// Parses a gettext PO catalog and merges its entries into `locale`. The header
    /// (empty `msgid`) and untranslated (empty `msgstr`) entries are skipped. Errors
    /// on malformed PO or a translation whose `{placeholders}` are not a subset of its
    /// source's, so a bad catalog fails boot like a bad descriptor key. Launch locales
    /// use `msgstr[0]`/`msgstr[1]` for the one/other plural forms.
    pub fn po(mut self, locale: &str, text: &str) -> Result<Self, String> {
        for entry in parse_po_entries(text)? {
            if entry.id.is_empty() {
                continue; // the PO header
            }
            let mut source = placeholder_set(&entry.id);
            if let Some(p) = &entry.id_plural {
                source.extend(placeholder_set(p));
            }
            let stored = if entry.id_plural.is_some() {
                let one = entry.plural_forms.first().cloned().unwrap_or_default();
                let other = entry.plural_forms.get(1).cloned().unwrap_or_default();
                if one.is_empty() && other.is_empty() {
                    continue; // untranslated plural
                }
                if one.is_empty() || other.is_empty() {
                    return Err(format!(
                        "`{}`: a plural needs msgstr[0] and msgstr[1]",
                        entry.id
                    ));
                }
                validate_placeholders(&one, &source, &entry.id)?;
                validate_placeholders(&other, &source, &entry.id)?;
                Entry::Plural { one, other }
            } else {
                match entry.singular {
                    Some(s) if !s.is_empty() => {
                        validate_placeholders(&s, &source, &entry.id)?;
                        Entry::One(s)
                    }
                    _ => continue, // untranslated
                }
            };
            self.insert(
                locale,
                catalog_key(&entry.id, entry.context.as_deref()),
                stored,
            );
        }
        Ok(self)
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

/// The `{name}` placeholders in `s` (ignoring `{{`/`}}`), for validating a
/// translation against its source form.
fn placeholder_set(s: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '{' => {
                let mut name = String::new();
                for ch in chars.by_ref() {
                    if ch == '}' {
                        break;
                    }
                    name.push(ch);
                }
                if !name.is_empty() {
                    out.insert(name);
                }
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
            }
            _ => {}
        }
    }
    out
}

/// Errors if `translation` uses a `{placeholder}` its `source` form does not have.
fn validate_placeholders(
    translation: &str,
    source: &std::collections::BTreeSet<String>,
    id: &str,
) -> Result<(), String> {
    for p in placeholder_set(translation) {
        if !source.contains(&p) {
            return Err(format!(
                "`{id}`: translation uses unknown placeholder {{{p}}}"
            ));
        }
    }
    Ok(())
}

/// One parsed PO entry, before it becomes a catalog [`Entry`].
#[derive(Default)]
struct PoEntry {
    context: Option<String>,
    id: String,
    id_plural: Option<String>,
    singular: Option<String>,
    plural_forms: Vec<String>,
}

/// The field a bare continuation string (`"..."`) appends to.
enum PoField {
    None,
    Context,
    Id,
    IdPlural,
    Singular,
    Plural(usize),
}

/// Parses PO text into entries: a minimal gettext subset (`msgctxt`/`msgid`/
/// `msgid_plural`/`msgstr`/`msgstr[n]`, adjacent-string continuation, `#` comments,
/// blank-line separators). Enough to load a derived catalog, not to author one.
fn parse_po_entries(text: &str) -> Result<Vec<PoEntry>, String> {
    let mut entries = Vec::new();
    let mut cur = PoEntry::default();
    let mut have = false;
    let mut field = PoField::None;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let at = |m: &str| format!("PO line {}: {m}", i + 1);
        if line.is_empty() {
            if have {
                entries.push(std::mem::take(&mut cur));
                have = false;
                field = PoField::None;
            }
        } else if line.starts_with('#') {
            continue;
        } else if let Some(rest) = line.strip_prefix("msgctxt ") {
            cur.context = Some(unquote(rest).map_err(|e| at(&e))?);
            have = true;
            field = PoField::Context;
        } else if let Some(rest) = line.strip_prefix("msgid_plural ") {
            cur.id_plural = Some(unquote(rest).map_err(|e| at(&e))?);
            have = true;
            field = PoField::IdPlural;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if have && !cur.id.is_empty() {
                entries.push(std::mem::take(&mut cur));
            }
            cur.id = unquote(rest).map_err(|e| at(&e))?;
            have = true;
            field = PoField::Id;
        } else if let Some(rest) = line.strip_prefix("msgstr[") {
            let (idx, q) = rest
                .split_once(']')
                .ok_or_else(|| at("malformed msgstr[n]"))?;
            let n: usize = idx.trim().parse().map_err(|_| at("bad plural index"))?;
            let s = unquote(q.trim()).map_err(|e| at(&e))?;
            if cur.plural_forms.len() <= n {
                cur.plural_forms.resize(n + 1, String::new());
            }
            cur.plural_forms[n] = s;
            have = true;
            field = PoField::Plural(n);
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            cur.singular = Some(unquote(rest).map_err(|e| at(&e))?);
            have = true;
            field = PoField::Singular;
        } else if line.starts_with('"') {
            let s = unquote(line).map_err(|e| at(&e))?;
            match field {
                PoField::Context => cur.context.get_or_insert_with(String::new).push_str(&s),
                PoField::Id => cur.id.push_str(&s),
                PoField::IdPlural => cur.id_plural.get_or_insert_with(String::new).push_str(&s),
                PoField::Singular => cur.singular.get_or_insert_with(String::new).push_str(&s),
                PoField::Plural(n) => cur.plural_forms[n].push_str(&s),
                PoField::None => return Err(at("string continuation with no field")),
            }
        } else {
            return Err(at("unrecognized PO line"));
        }
    }
    if have {
        entries.push(cur);
    }
    Ok(entries)
}

/// Extracts and unescapes a PO quoted string (`"..."`), handling `\n \t \r \" \\`.
fn unquote(s: &str) -> Result<String, String> {
    let inner = s
        .trim()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| "expected a quoted string".to_string())?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => return Err("dangling escape in a PO string".to_string()),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
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
    /// interpolates its `{name}` arguments. Under the pseudo-locale the resolved
    /// template is accented and bracketed first.
    pub fn t(&self, text: &Text) -> String {
        let key = catalog_key(&text.source, text.context.as_deref());
        let template = self.resolve(&key, text);
        let template = if self.locale() == PSEUDO_LOCALE {
            pseudo(&template)
        } else {
            template
        };
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

/// Pseudo-localizes a resolved template for [`PSEUDO_LOCALE`]: accents letters and
/// wraps the whole in brackets. `{name}` placeholders (and `{{`/`}}` literals) are
/// copied verbatim so interpolation still matches and the args stay real data.
fn pseudo(template: &str) -> String {
    let mut out = String::with_capacity(template.len() + 6);
    out.push('\u{27E6}'); // (left white square bracket)
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                out.push(c);
                out.push(chars.next().unwrap());
            }
            '{' => {
                out.push(c);
                for ch in chars.by_ref() {
                    out.push(ch);
                    if ch == '}' {
                        break;
                    }
                }
            }
            _ => out.push(accent(c)),
        }
    }
    out.push('\u{27E7}'); // (right white square bracket)
    out
}

/// Maps a vowel to an accented form for the pseudo-locale; other characters pass
/// through. Vowels alone make the text visibly non-English while staying readable.
fn accent(c: char) -> char {
    match c {
        'a' => '\u{e4}',
        'e' => '\u{e9}',
        'i' => '\u{ed}',
        'o' => '\u{f6}',
        'u' => '\u{fc}',
        'A' => '\u{c4}',
        'E' => '\u{c9}',
        'I' => '\u{cd}',
        'O' => '\u{d6}',
        'U' => '\u{dc}',
        other => other,
    }
}

/// The newtype name a plain [`Text`] serializes under, so a collector can spot it.
const TEXT_MARKER: &str = "laterite.Text";

/// Harvests the source string of every plain [`Text`] reachable in `value`, by
/// intercepting the `laterite.Text` newtype marker its `Serialize` emits. This lets
/// `lat i18n extract` collect the label strings that live in descriptor *data*
/// (nav/title/column/permission/settings labels) without the collector knowing the
/// descriptor shapes, so it cannot drift as new label fields are added. Descriptor
/// labels are plain `Text`; a rich `Text` (context/plural/args) does not appear in a
/// descriptor and is not collected here.
pub fn collect_sources<T: Serialize>(value: &T) -> Vec<String> {
    let mut out = Vec::new();
    let _ = value.serialize(SourceCollector {
        out: &mut out,
        capture: false,
    });
    out
}

/// The (unreachable) error type for the infallible collector serializer.
#[derive(Debug)]
pub struct CollectError;
impl fmt::Display for CollectError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("source collector error")
    }
}
impl std::error::Error for CollectError {}
impl serde::ser::Error for CollectError {
    fn custom<T: fmt::Display>(_: T) -> Self {
        CollectError
    }
}

/// A serializer that walks a value and records `Text` sources. `capture` is set only
/// for the string inside a `laterite.Text` newtype, so a plain data string elsewhere
/// (an entity name, a path) is walked but never recorded.
struct SourceCollector<'a> {
    out: &'a mut Vec<String>,
    capture: bool,
}

impl<'a> SourceCollector<'a> {
    fn child(&mut self) -> SourceCollector<'_> {
        SourceCollector {
            out: self.out,
            capture: false,
        }
    }
}

impl<'a> Serializer for SourceCollector<'a> {
    type Ok = ();
    type Error = CollectError;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    fn serialize_str(self, v: &str) -> Result<(), CollectError> {
        if self.capture {
            self.out.push(v.to_string());
        }
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<(), CollectError> {
        let collector = SourceCollector {
            out: self.out,
            capture: name == TEXT_MARKER,
        };
        value.serialize(collector)
    }

    fn serialize_some<T: ?Sized + Serialize>(mut self, value: &T) -> Result<(), CollectError> {
        value.serialize(self.child())
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        mut self,
        _n: &'static str,
        _i: u32,
        _v: &'static str,
        value: &T,
    ) -> Result<(), CollectError> {
        value.serialize(self.child())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Compound<'a>, CollectError> {
        Ok(Compound { out: self.out })
    }
    fn serialize_tuple(self, _len: usize) -> Result<Compound<'a>, CollectError> {
        Ok(Compound { out: self.out })
    }
    fn serialize_tuple_struct(
        self,
        _n: &'static str,
        _len: usize,
    ) -> Result<Compound<'a>, CollectError> {
        Ok(Compound { out: self.out })
    }
    fn serialize_tuple_variant(
        self,
        _n: &'static str,
        _i: u32,
        _v: &'static str,
        _len: usize,
    ) -> Result<Compound<'a>, CollectError> {
        Ok(Compound { out: self.out })
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Compound<'a>, CollectError> {
        Ok(Compound { out: self.out })
    }
    fn serialize_struct(self, _n: &'static str, _len: usize) -> Result<Compound<'a>, CollectError> {
        Ok(Compound { out: self.out })
    }
    fn serialize_struct_variant(
        self,
        _n: &'static str,
        _i: u32,
        _v: &'static str,
        _len: usize,
    ) -> Result<Compound<'a>, CollectError> {
        Ok(Compound { out: self.out })
    }

    // Scalars and unit-like values carry no `Text`; walk over them.
    fn serialize_bool(self, _v: bool) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_i8(self, _v: i8) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_i16(self, _v: i16) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_i32(self, _v: i32) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_i64(self, _v: i64) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_u8(self, _v: u8) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_u16(self, _v: u16) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_u32(self, _v: u32) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_u64(self, _v: u64) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_f32(self, _v: f32) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_f64(self, _v: f64) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_char(self, _v: char) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_none(self) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_unit(self) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_unit_struct(self, _n: &'static str) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _n: &'static str,
        _i: u32,
        _v: &'static str,
    ) -> Result<(), CollectError> {
        Ok(())
    }
}

/// The one compound-serializer type all of `Serializer`'s associated types share:
/// it recurses each element/value with a fresh collector and ignores keys.
struct Compound<'a> {
    out: &'a mut Vec<String>,
}

impl Compound<'_> {
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), CollectError> {
        value.serialize(SourceCollector {
            out: self.out,
            capture: false,
        })
    }
}

impl SerializeSeq for Compound<'_> {
    type Ok = ();
    type Error = CollectError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), CollectError> {
        self.element(v)
    }
    fn end(self) -> Result<(), CollectError> {
        Ok(())
    }
}
impl SerializeTuple for Compound<'_> {
    type Ok = ();
    type Error = CollectError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), CollectError> {
        self.element(v)
    }
    fn end(self) -> Result<(), CollectError> {
        Ok(())
    }
}
impl SerializeTupleStruct for Compound<'_> {
    type Ok = ();
    type Error = CollectError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), CollectError> {
        self.element(v)
    }
    fn end(self) -> Result<(), CollectError> {
        Ok(())
    }
}
impl SerializeTupleVariant for Compound<'_> {
    type Ok = ();
    type Error = CollectError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), CollectError> {
        self.element(v)
    }
    fn end(self) -> Result<(), CollectError> {
        Ok(())
    }
}
impl SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = CollectError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, _k: &T) -> Result<(), CollectError> {
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), CollectError> {
        self.element(v)
    }
    fn end(self) -> Result<(), CollectError> {
        Ok(())
    }
}
impl SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = CollectError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _k: &'static str,
        v: &T,
    ) -> Result<(), CollectError> {
        self.element(v)
    }
    fn end(self) -> Result<(), CollectError> {
        Ok(())
    }
}
impl SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = CollectError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _k: &'static str,
        v: &T,
    ) -> Result<(), CollectError> {
        self.element(v)
    }
    fn end(self) -> Result<(), CollectError> {
        Ok(())
    }
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
    fn collect_sources_finds_text_but_not_plain_strings() {
        // A struct mixing plain data strings with Text labels; only the Text sources
        // are collected, and nested/optional Text is found too.
        #[derive(Serialize)]
        struct Field {
            name: String,
            label: Text,
        }
        #[derive(Serialize)]
        struct Descriptor {
            entity: String,
            title: Text,
            fields: Vec<Field>,
            help: Option<Text>,
        }
        let d = Descriptor {
            entity: "widgets".into(),
            title: Text::dynamic("Widgets"),
            fields: vec![Field {
                name: "code".into(),
                label: Text::dynamic("Code"),
            }],
            help: Some(Text::dynamic("Pick one")),
        };
        let mut got = collect_sources(&d);
        got.sort();
        assert_eq!(got, vec!["Code", "Pick one", "Widgets"]);
    }

    #[test]
    fn po_catalog_loads_messages_context_and_plurals() {
        let po = concat!(
            "# a comment\n",
            "msgid \"\"\n",
            "msgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n",
            "\n",
            "msgid \"Save\"\n",
            "msgstr \"Save-kn\"\n",
            "\n",
            "msgctxt \"verb\"\n",
            "msgid \"Open\"\n",
            "msgstr \"Open-verb-kn\"\n",
            "\n",
            "msgid \"{n} item\"\n",
            "msgid_plural \"{n} items\"\n",
            "msgstr[0] \"{n} one-kn\"\n",
            "msgstr[1] \"{n} other-kn\"\n",
        );
        let store = Arc::new(CatalogStore::builder().po("kn", po).unwrap().build());
        let tr = Translator::with_chain(vec!["kn".into(), "en".into()], store);
        assert_eq!(tr.t(&Text::new("Save")), "Save-kn");
        assert_eq!(
            tr.t(&Text::new("Open").with_context("verb")),
            "Open-verb-kn"
        );
        // A different (or absent) context is a different key: it falls back.
        assert_eq!(tr.t(&Text::new("Open")), "Open");
        let items = |n: i64| {
            tr.t(&Text::new("{n} item")
                .with_plural("{n} items", n)
                .arg("n", n))
        };
        assert_eq!(items(1), "1 one-kn");
        assert_eq!(items(3), "3 other-kn");
    }

    #[test]
    fn po_rejects_a_translation_with_an_unknown_placeholder() {
        let po = "msgid \"Hi {name}\"\nmsgstr \"Hi {nom}\"\n";
        assert!(CatalogStore::builder().po("kn", po).is_err());
    }

    #[test]
    fn po_skips_header_and_untranslated_entries() {
        let po = "msgid \"\"\nmsgstr \"meta\"\n\nmsgid \"Later\"\nmsgstr \"\"\n";
        let store = Arc::new(CatalogStore::builder().po("kn", po).unwrap().build());
        let tr = Translator::with_chain(vec!["kn".into(), "en".into()], store);
        // The header is not a key, and an empty translation leaves the source to fall back.
        assert_eq!(tr.t(&Text::new("Later")), "Later");
    }

    #[test]
    fn po_handles_string_continuation_and_escapes() {
        let po = "msgid \"Long\"\nmsgstr \"\"\n\"line one\\n\"\n\"line two\"\n";
        let store = Arc::new(CatalogStore::builder().po("kn", po).unwrap().build());
        let tr = Translator::with_chain(vec!["kn".into(), "en".into()], store);
        assert_eq!(tr.t(&Text::new("Long")), "line one\nline two");
    }

    #[test]
    fn pseudo_locale_accents_wraps_and_preserves_placeholders() {
        let t = Translator::new(PSEUDO_LOCALE);
        // A plain source is bracketed and its vowels accented.
        assert_eq!(t.t(&Text::new("Save")), "\u{27E6}S\u{e4}v\u{e9}\u{27E7}");
        // The placeholder is copied verbatim, so the arg still interpolates and the
        // arg value (real data) is not accented.
        let out = t.t(&Text::new("Hi {name}").arg("name", "Asha"));
        assert_eq!(out, "\u{27E6}H\u{ed} Asha\u{27E7}");
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
