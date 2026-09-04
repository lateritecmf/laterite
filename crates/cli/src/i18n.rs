//! `lat i18n`: derive, check, and manage the message catalogs.
//!
//! `extract` scans a workspace's Rust (`t!`/`tn!`/`tp!` macros, plus core's
//! `field_msg`/`field_plural_msg`), templates (`shell.t`/`shell.tf`/`shell.tfs`), and
//! the built-in descriptor sources, and writes a deterministic gettext `.pot` per
//! crate under `<crate>/lang/`. `check` regenerates in memory and fails if a committed
//! `.pot` is stale, so the catalog never drifts from the code. `update <locale>`
//! merges the `.pot` into `<crate>/lang/<locale>.po`, keeping existing translations
//! and adding new empty ones (creating the file if absent). `status` reports per-locale
//! coverage. The English source is the key, so the `.pot` is generated, never
//! hand-edited; only a locale `.po`'s `msgstr` lines are edited, by translators.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct I18nArgs {
    #[command(subcommand)]
    command: I18nCommand,
}

#[derive(Subcommand)]
enum I18nCommand {
    /// Regenerate each crate's lang/messages.pot from its source strings.
    Extract,
    /// Fail if any committed messages.pot is out of date (CI / the verify loop).
    Check,
    /// Merge each crate's messages.pot into a locale's .po, keeping existing
    /// translations and adding new (empty) entries. Creates the .po if absent.
    Update {
        /// The locale to update or create (e.g. `kn`, `hi`, `ta`).
        locale: String,
    },
    /// Report translation coverage for each crate's locale catalogs.
    Status,
}

pub fn run(args: I18nArgs) -> Result<()> {
    let root = Path::new(".");
    match args.command {
        I18nCommand::Extract => extract(root),
        I18nCommand::Check => check(root),
        I18nCommand::Update { locale } => update(root, &locale),
        I18nCommand::Status => status(root),
    }
}

/// The key of a message: its optional context and its source (English) string.
type Key = (Option<String>, String);

/// A collected message: its plural source form (if any) and where it appears.
#[derive(Default)]
struct Entry {
    plural: Option<String>,
    refs: Vec<String>,
}

/// The messages of one crate, ordered by (context, source) for a stable `.pot`.
type Catalog = BTreeMap<Key, Entry>;

fn extract(root: &Path) -> Result<()> {
    for krate in crates(root)? {
        let cat = collect(&krate)?;
        let pot = krate.join("lang").join("messages.pot");
        if cat.is_empty() {
            continue;
        }
        fs::create_dir_all(pot.parent().unwrap())?;
        let text = render(&cat, &Translations::new(), POT_HEADER);
        fs::write(&pot, text).with_context(|| format!("writing {}", pot.display()))?;
        println!("{}: {} messages", pot.display(), cat.len());
    }
    Ok(())
}

fn check(root: &Path) -> Result<()> {
    let mut stale = Vec::new();
    for krate in crates(root)? {
        let cat = collect(&krate)?;
        let pot = krate.join("lang").join("messages.pot");
        let want = if cat.is_empty() {
            String::new()
        } else {
            render(&cat, &Translations::new(), POT_HEADER)
        };
        let have = fs::read_to_string(&pot).unwrap_or_default();
        if have != want {
            stale.push(pot.display().to_string());
        }
    }
    if !stale.is_empty() {
        bail!(
            "stale message catalog(s); run `lat i18n extract`:\n  {}",
            stale.join("\n  ")
        );
    }
    println!("i18n catalogs are up to date.");
    Ok(())
}

fn update(root: &Path, locale: &str) -> Result<()> {
    for krate in crates(root)? {
        let cat = collect(&krate)?;
        if cat.is_empty() {
            continue;
        }
        let po = krate.join("lang").join(format!("{locale}.po"));
        let existing = parse_po(&fs::read_to_string(&po).unwrap_or_default());
        let kept = cat.keys().filter(|k| existing.contains_key(*k)).count();
        let dropped = existing.keys().filter(|k| !cat.contains_key(k)).count();
        fs::create_dir_all(po.parent().unwrap())?;
        let header = format!(
            "{locale} translations; keep msgid, translate msgstr. `lat i18n update` refreshes."
        );
        fs::write(&po, render(&cat, &existing, &header))
            .with_context(|| format!("writing {}", po.display()))?;
        println!(
            "{}: {kept} kept, {} new, {dropped} dropped",
            po.display(),
            cat.len() - kept
        );
    }
    Ok(())
}

fn status(root: &Path) -> Result<()> {
    for krate in crates(root)? {
        let cat = collect(&krate)?;
        if cat.is_empty() {
            continue;
        }
        let name = krate.file_name().unwrap_or_default().to_string_lossy();
        let total = cat.len();
        let mut locales: Vec<PathBuf> = files(&krate.join("lang"), "po");
        locales.sort();
        if locales.is_empty() {
            println!("{name}: {total} messages, no locale catalogs yet");
        }
        for po in locales {
            let loc = po.file_stem().unwrap_or_default().to_string_lossy();
            let tr = parse_po(&fs::read_to_string(&po).unwrap_or_default());
            let done = cat.keys().filter(|k| tr.contains_key(*k)).count();
            let pct = (done * 100).checked_div(total).unwrap_or(100);
            println!("{name}/{loc}: {done}/{total} ({pct}%)");
        }
    }
    Ok(())
}

/// An accumulator for one `.po` entry while parsing line by line.
#[derive(Default)]
struct PoAcc {
    context: Option<String>,
    id: Option<String>,
    id_plural: Option<String>,
    single: Option<String>,
    plural: Vec<String>,
}

/// Records a finished `.po` entry (skipping the header and untranslated ones).
fn flush_po(acc: &mut PoAcc, out: &mut Translations) {
    let acc = std::mem::take(acc);
    let Some(id) = acc.id.filter(|s| !s.is_empty()) else {
        return;
    };
    let key = (acc.context, id);
    if acc.id_plural.is_some() {
        let one = acc.plural.first().cloned().unwrap_or_default();
        let other = acc.plural.get(1).cloned().unwrap_or_default();
        if !one.is_empty() || !other.is_empty() {
            out.insert(key, PoValue::Plural(one, other));
        }
    } else if let Some(s) = acc.single.filter(|s| !s.is_empty()) {
        out.insert(key, PoValue::One(s));
    }
}

/// Reads a `.po` into its non-empty translations, keyed by (context, msgid). A
/// minimal gettext reader: the `msgctxt`/`msgid`/`msgid_plural`/`msgstr`/`msgstr[n]`
/// keywords with adjacent-string continuation; comments and the header are skipped.
fn parse_po(text: &str) -> Translations {
    let mut out = Translations::new();
    let mut acc = PoAcc::default();
    let mut field = PoField::None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            flush_po(&mut acc, &mut out);
            field = PoField::None;
        } else if line.starts_with('#') {
            continue;
        } else if let Some(rest) = line.strip_prefix("msgctxt ") {
            acc.context = po_unquote(rest);
            field = PoField::Context;
        } else if let Some(rest) = line.strip_prefix("msgid_plural ") {
            acc.id_plural = po_unquote(rest);
            field = PoField::IdPlural;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if acc.id.is_some() {
                flush_po(&mut acc, &mut out);
            }
            acc.id = po_unquote(rest);
            field = PoField::Id;
        } else if let Some(rest) = line.strip_prefix("msgstr[") {
            if let Some((idx, q)) = rest.split_once(']') {
                if let (Ok(n), Some(s)) = (idx.trim().parse::<usize>(), po_unquote(q.trim())) {
                    if acc.plural.len() <= n {
                        acc.plural.resize(n + 1, String::new());
                    }
                    acc.plural[n] = s;
                    field = PoField::Plural(n);
                }
            }
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            acc.single = po_unquote(rest);
            field = PoField::Single;
        } else if line.starts_with('"') {
            if let Some(s) = po_unquote(line) {
                match field {
                    PoField::Context => append(&mut acc.context, &s),
                    PoField::Id => append(&mut acc.id, &s),
                    PoField::IdPlural => append(&mut acc.id_plural, &s),
                    PoField::Single => append(&mut acc.single, &s),
                    PoField::Plural(n) => acc.plural[n].push_str(&s),
                    PoField::None => {}
                }
            }
        }
    }
    flush_po(&mut acc, &mut out);
    out
}

enum PoField {
    None,
    Context,
    Id,
    IdPlural,
    Single,
    Plural(usize),
}

fn append(field: &mut Option<String>, s: &str) {
    field.get_or_insert_with(String::new).push_str(s);
}

/// Reads a `"..."` gettext string, unescaping `\n \t \r \" \\`; `None` if unquoted.
fn po_unquote(s: &str) -> Option<String> {
    let inner = s.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// The crate directories to scan: each member of a `crates/` workspace, or the
/// root itself when it is a single crate.
fn crates(root: &Path) -> Result<Vec<PathBuf>> {
    let members = root.join("crates");
    if members.is_dir() {
        let mut out = Vec::new();
        for entry in fs::read_dir(&members)? {
            let dir = entry?.path();
            if dir.join("Cargo.toml").is_file() {
                out.push(dir);
            }
        }
        out.sort();
        Ok(out)
    } else if root.join("Cargo.toml").is_file() {
        Ok(vec![root.to_path_buf()])
    } else {
        bail!(
            "no crate found at {} (expected Cargo.toml or crates/)",
            root.display()
        );
    }
}

/// Scans one crate's `src/**/*.rs` and `templates/**/*.html` into a catalog.
fn collect(krate: &Path) -> Result<Catalog> {
    let mut cat = Catalog::new();
    for path in files(&krate.join("src"), "rs") {
        let src = fs::read_to_string(&path)?;
        scan_rust(&mut cat, &rel(krate, &path), &src)?;
    }
    for path in files(&krate.join("templates"), "html") {
        let src = fs::read_to_string(&path)?;
        scan_template(&mut cat, &rel(krate, &path), &src);
    }
    // The framework's built-in descriptor labels live in serde data, not in `t!` or
    // templates, so the admin crate folds in its runtime source walk.
    if krate.file_name().is_some_and(|n| n == "admin") {
        for source in laterite_admin::descriptor_sources() {
            add(&mut cat, None, source, None, "(descriptors)".to_string());
        }
    }
    Ok(cat)
}

fn rel(krate: &Path, path: &Path) -> String {
    path.strip_prefix(krate)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Every file with `ext` under `dir`, recursively, in sorted order.
fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, ext, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

fn add(
    cat: &mut Catalog,
    context: Option<String>,
    source: String,
    plural: Option<String>,
    r: String,
) {
    let entry = cat.entry((context, source)).or_default();
    if plural.is_some() {
        entry.plural = plural;
    }
    if !entry.refs.contains(&r) {
        entry.refs.push(r);
    }
}

// --- Rust: `t!` / `tn!` / `tp!` -------------------------------------------------

/// Collects the source strings of every `t!`/`tn!`/`tp!` in `src`.
fn scan_rust(cat: &mut Catalog, path: &str, src: &str) -> Result<()> {
    let file = syn::parse_file(src).with_context(|| format!("parsing {path}"))?;
    let mut visitor = Macros { cat, path };
    syn::visit::visit_file(&mut visitor, &file);
    Ok(())
}

struct Macros<'a> {
    cat: &'a mut Catalog,
    path: &'a str,
}

impl syn::visit::Visit<'_> for Macros<'_> {
    fn visit_macro(&mut self, mac: &syn::Macro) {
        let name = mac.path.segments.last().map(|s| s.ident.to_string());
        let line = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.span().start().line)
            .unwrap_or(0);
        let r = format!("{}:{line}", self.path);
        match name.as_deref() {
            Some("t") => {
                if let Ok(one) = syn::parse2::<OneLit>(mac.tokens.clone()) {
                    add(self.cat, None, one.0.value(), None, r);
                }
            }
            Some("tp") => {
                if let Ok(two) = syn::parse2::<TwoLit>(mac.tokens.clone()) {
                    add(self.cat, Some(two.0.value()), two.1.value(), None, r);
                }
            }
            Some("tn") => {
                if let Ok(two) = syn::parse2::<TwoLit>(mac.tokens.clone()) {
                    add(self.cat, None, two.0.value(), Some(two.1.value()), r);
                }
            }
            _ => {}
        }
        syn::visit::visit_macro(self, mac);
    }

    fn visit_expr_call(&mut self, call: &syn::ExprCall) {
        // `core` cannot invoke `t!` (the macro expands to `::laterite_core::...`), so
        // it builds validation messages through the helpers `field_msg(source, ...)`
        // and `field_plural_msg(one, other, ...)`. Treat those like `t!`/`tn!`.
        if let syn::Expr::Path(path) = &*call.func {
            if let Some(seg) = path.path.segments.last() {
                let r = format!("{}:{}", self.path, seg.ident.span().start().line);
                let lit = |i: usize| match call.args.get(i) {
                    Some(syn::Expr::Lit(l)) => match &l.lit {
                        syn::Lit::Str(s) => Some(s.value()),
                        _ => None,
                    },
                    _ => None,
                };
                match seg.ident.to_string().as_str() {
                    "field_msg" => {
                        if let Some(s) = lit(0) {
                            add(self.cat, None, s, None, r);
                        }
                    }
                    "field_plural_msg" => {
                        if let (Some(one), Some(other)) = (lit(0), lit(1)) {
                            add(self.cat, None, one, Some(other), r);
                        }
                    }
                    _ => {}
                }
            }
        }
        syn::visit::visit_expr_call(self, call);
    }
}

/// The leading string literal of a macro invocation (`t!("x", ...)`).
struct OneLit(syn::LitStr);
impl syn::parse::Parse for OneLit {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let lit = input.parse()?;
        input.parse::<proc_macro2::TokenStream>()?; // ignore the rest
        Ok(OneLit(lit))
    }
}

/// The two leading string literals (`tp!("ctx", "x")`, `tn!("one", "other", ...)`).
struct TwoLit(syn::LitStr, syn::LitStr);
impl syn::parse::Parse for TwoLit {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let first = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let second = input.parse()?;
        input.parse::<proc_macro2::TokenStream>()?; // ignore the rest
        Ok(TwoLit(first, second))
    }
}

// --- Templates: `shell.t(` / `shell.tf(` / `shell.tfs(` -------------------------

/// Collects the leading string literal of every `shell.t`/`tf`/`tfs` call in a
/// template. `shell.tt` takes a prebuilt `Text`, so it carries no literal to scan.
fn scan_template(cat: &mut Catalog, path: &str, src: &str) {
    for needle in ["shell.t(", "shell.tf(", "shell.tfs("] {
        let mut from = 0;
        while let Some(pos) = src[from..].find(needle) {
            let call = from + pos;
            from = call + needle.len();
            if let Some(source) = string_after(&src[from..]) {
                let line = src[..call].bytes().filter(|&b| b == b'\n').count() + 1;
                add(cat, None, source, None, format!("{path}:{line}"));
            }
        }
    }
}

/// Reads a `"..."` literal at the start of `s` (after optional whitespace),
/// unescaping `\"` and `\\`. Returns `None` if the next token is not a string.
fn string_after(s: &str) -> Option<String> {
    let s = s.trim_start();
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            c => out.push(c),
        }
    }
}

// --- PO rendering ---------------------------------------------------------------

/// A translation read from a `.po`: a single form, or the two plural forms.
enum PoValue {
    One(String),
    Plural(String, String),
}

/// Existing translations, keyed like a [`Catalog`].
type Translations = BTreeMap<Key, PoValue>;

/// Renders a catalog as a deterministic gettext catalog, filling each `msgstr` from
/// `tr` (empty when a message is untranslated). An empty `tr` yields a `.pot`.
fn render(cat: &Catalog, tr: &Translations, header: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {header}");
    out.push_str("msgid \"\"\n");
    out.push_str("msgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n");
    for (key @ (context, source), entry) in cat {
        out.push('\n');
        for r in &entry.refs {
            let _ = writeln!(out, "#: {r}");
        }
        if let Some(context) = context {
            let _ = writeln!(out, "msgctxt \"{}\"", escape(context));
        }
        let _ = writeln!(out, "msgid \"{}\"", escape(source));
        let value = tr.get(key);
        match &entry.plural {
            Some(plural) => {
                let _ = writeln!(out, "msgid_plural \"{}\"", escape(plural));
                let (one, other) = match value {
                    Some(PoValue::Plural(a, b)) => (a.as_str(), b.as_str()),
                    _ => ("", ""),
                };
                let _ = writeln!(out, "msgstr[0] \"{}\"", escape(one));
                let _ = writeln!(out, "msgstr[1] \"{}\"", escape(other));
            }
            None => {
                let one = match value {
                    Some(PoValue::One(s)) => s.as_str(),
                    _ => "",
                };
                let _ = writeln!(out, "msgstr \"{}\"", escape(one));
            }
        }
    }
    out
}

/// The template header for a `.pot`.
const POT_HEADER: &str = "Generated by `lat i18n extract`; do not edit.";

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_rust_macros_with_context_and_plurals() {
        let mut cat = Catalog::new();
        let src = r#"
            fn f() {
                let _ = t!("Save");
                let _ = t!("Welcome, {name}", name = who);
                let _ = tp!("verb", "Open");
                let _ = tn!("{n} item", "{n} items", n = count);
            }
        "#;
        scan_rust(&mut cat, "src/f.rs", src).unwrap();
        assert!(cat.contains_key(&(None, "Save".to_string())));
        assert!(cat.contains_key(&(None, "Welcome, {name}".to_string())));
        assert!(cat.contains_key(&(Some("verb".to_string()), "Open".to_string())));
        let plural = &cat[&(None, "{n} item".to_string())];
        assert_eq!(plural.plural.as_deref(), Some("{n} items"));
    }

    #[test]
    fn scans_core_message_helpers() {
        let mut cat = Catalog::new();
        let src = r#"
            fn f() {
                bag.add(&x, field_msg("{label} is required.", &label));
                let _ = field_plural_msg("{n} char", "{n} chars", n, &label);
            }
        "#;
        scan_rust(&mut cat, "src/validation.rs", src).unwrap();
        assert!(cat.contains_key(&(None, "{label} is required.".to_string())));
        let plural = &cat[&(None, "{n} char".to_string())];
        assert_eq!(plural.plural.as_deref(), Some("{n} chars"));
    }

    #[test]
    fn scans_template_shell_calls() {
        let mut cat = Catalog::new();
        let src = r#"<h1>{{ shell.t("Dashboard") }}</h1>
            <p>{{ shell.tf("Page {n}", [("n", page)]) }}</p>
            <span>{{ shell.tfs("Hi {name}", [("name", u)]) }}</span>
            <i>{{ shell.tt(msg) }}</i>"#;
        scan_template(&mut cat, "templates/x.html", src);
        assert!(cat.contains_key(&(None, "Dashboard".to_string())));
        assert!(cat.contains_key(&(None, "Page {n}".to_string())));
        assert!(cat.contains_key(&(None, "Hi {name}".to_string())));
        // shell.tt has no literal source, so nothing is collected for it.
        assert_eq!(cat.len(), 3);
    }

    #[test]
    fn update_carries_existing_translations_forward() {
        let mut cat = Catalog::new();
        add(&mut cat, None, "Save".to_string(), None, "a:1".to_string());
        add(
            &mut cat,
            None,
            "{n} item".to_string(),
            Some("{n} items".to_string()),
            "a:2".to_string(),
        );
        // A .po with one translated singular and an untranslated plural.
        let po = "msgid \"Save\"\nmsgstr \"Save-kn\"\n\n\
                  msgid \"{n} item\"\nmsgid_plural \"{n} items\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
        let tr = parse_po(po);
        assert!(
            matches!(tr.get(&(None, "Save".to_string())), Some(PoValue::One(s)) if s == "Save-kn")
        );
        // The untranslated plural was empty, so it is not carried.
        assert!(!tr.contains_key(&(None, "{n} item".to_string())));
        // Rendering keeps the translation and re-parses to the same value.
        let rendered = render(&cat, &tr, "hdr");
        assert!(rendered.contains("msgid \"Save\"\nmsgstr \"Save-kn\"\n"));
        assert!(rendered.contains("msgstr[0] \"\"\nmsgstr[1] \"\"\n"));
        assert!(parse_po(&rendered).contains_key(&(None, "Save".to_string())));
    }

    #[test]
    fn renders_a_deterministic_pot() {
        let mut cat = Catalog::new();
        add(
            &mut cat,
            None,
            "Save".to_string(),
            None,
            "src/a.rs:2".to_string(),
        );
        add(
            &mut cat,
            None,
            "{n} item".to_string(),
            Some("{n} items".to_string()),
            "src/a.rs:3".to_string(),
        );
        let pot = render(&cat, &Translations::new(), POT_HEADER);
        assert!(pot.contains("msgid \"Save\"\nmsgstr \"\"\n"));
        assert!(pot.contains(
            "msgid \"{n} item\"\nmsgid_plural \"{n} items\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n"
        ));
        // Deterministic: the same catalog renders identically.
        assert_eq!(render(&cat, &Translations::new(), POT_HEADER), pot);
    }
}
