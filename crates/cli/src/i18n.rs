//! `lat i18n`: derive and check the message catalogs.
//!
//! `extract` scans a workspace's Rust (`t!`/`tn!`/`tp!` macros) and templates
//! (`shell.t`/`shell.tf`/`shell.tfs`) for source strings and writes a deterministic
//! gettext `.pot` per crate under `<crate>/lang/`. `check` regenerates in memory and
//! fails if a committed `.pot` is stale, so the catalog never drifts from the code.
//! The English source is the key, so the `.pot` is generated, never hand-edited.

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
}

pub fn run(args: I18nArgs) -> Result<()> {
    let root = Path::new(".");
    match args.command {
        I18nCommand::Extract => extract(root),
        I18nCommand::Check => check(root),
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
        fs::write(&pot, render(&cat)).with_context(|| format!("writing {}", pot.display()))?;
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
            render(&cat)
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

/// Renders a catalog as a deterministic gettext `.pot` (empty translations).
fn render(cat: &Catalog) -> String {
    let mut out = String::new();
    out.push_str("# Generated by `lat i18n extract`; do not edit.\n");
    out.push_str("msgid \"\"\n");
    out.push_str("msgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n");
    for ((context, source), entry) in cat {
        out.push('\n');
        for r in &entry.refs {
            let _ = writeln!(out, "#: {r}");
        }
        if let Some(context) = context {
            let _ = writeln!(out, "msgctxt \"{}\"", escape(context));
        }
        let _ = writeln!(out, "msgid \"{}\"", escape(source));
        match &entry.plural {
            Some(plural) => {
                let _ = writeln!(out, "msgid_plural \"{}\"", escape(plural));
                out.push_str("msgstr[0] \"\"\n");
                out.push_str("msgstr[1] \"\"\n");
            }
            None => out.push_str("msgstr \"\"\n"),
        }
    }
    out
}

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
        let pot = render(&cat);
        assert!(pot.contains("msgid \"Save\"\nmsgstr \"\"\n"));
        assert!(pot.contains(
            "msgid \"{n} item\"\nmsgid_plural \"{n} items\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n"
        ));
        // Ordered by key: "Save" sorts after "{n} item" (brace < S), so both present.
        assert_eq!(render(&cat), pot);
    }
}
