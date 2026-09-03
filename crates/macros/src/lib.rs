//! Translation-string macros.
//!
//! `t!`, `tn!`, `tp!` build a `laterite_core::i18n::Text` at the call site. The
//! source is a string literal (so extraction can collect it), its `{name}`
//! placeholders are checked against the named arguments at compile time, and
//! positional `{}` or format specs are rejected. For a non-literal source, use
//! `Text::dynamic`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Expr, Ident, LitStr, Token};

/// A `name = expr` argument.
#[derive(Clone)]
struct KwArg {
    name: Ident,
    expr: Expr,
}

impl Parse for KwArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![=]>()?;
        let expr = input.parse()?;
        Ok(Self { name, expr })
    }
}

/// Parses trailing `, name = expr` arguments; a trailing comma is allowed.
fn trailing_kwargs(input: ParseStream) -> syn::Result<Vec<KwArg>> {
    let mut kwargs = Vec::new();
    while input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
        if input.is_empty() {
            break;
        }
        kwargs.push(input.parse()?);
    }
    Ok(kwargs)
}

struct T {
    source: LitStr,
    kwargs: Vec<KwArg>,
}
impl Parse for T {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let source = input.parse()?;
        Ok(Self {
            source,
            kwargs: trailing_kwargs(input)?,
        })
    }
}

struct Tp {
    context: LitStr,
    source: LitStr,
    kwargs: Vec<KwArg>,
}
impl Parse for Tp {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let context = input.parse()?;
        input.parse::<Token![,]>()?;
        let source = input.parse()?;
        Ok(Self {
            context,
            source,
            kwargs: trailing_kwargs(input)?,
        })
    }
}

struct Tn {
    one: LitStr,
    other: LitStr,
    kwargs: Vec<KwArg>,
}
impl Parse for Tn {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let one = input.parse()?;
        input.parse::<Token![,]>()?;
        let other = input.parse()?;
        Ok(Self {
            one,
            other,
            kwargs: trailing_kwargs(input)?,
        })
    }
}

fn compile_error(span: Span, msg: &str) -> TokenStream {
    syn::Error::new(span, msg).to_compile_error().into()
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The ordered, unique `{name}` placeholders in `s`, or an error message. `{{` and
/// `}}` are literal braces; `{}` and `{name:spec}` are rejected.
fn placeholders(s: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    continue;
                }
                let mut name = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    name.push(ch);
                }
                if !closed {
                    return Err("unmatched '{' in a translation string".into());
                }
                if name.is_empty() {
                    return Err("positional '{}' is not allowed; name the placeholder".into());
                }
                if name.contains(':') {
                    return Err(format!("format spec in '{{{name}}}' is not allowed"));
                }
                if !is_ident(&name) {
                    return Err(format!("'{{{name}}}' is not a valid placeholder name"));
                }
                if !out.iter().any(|p| p == &name) {
                    out.push(name);
                }
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                } else {
                    return Err(
                        "unmatched '}' in a translation string; use '}}' for a literal".into(),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Every placeholder needs an argument, and every argument must be used by a
/// placeholder except names listed in `extra` (`tn!`'s count `n`).
fn check(placeholders: &[String], names: &[String], extra: &[&str]) -> Result<(), String> {
    for p in placeholders {
        if !names.iter().any(|n| n == p) {
            return Err(format!(
                "placeholder '{{{p}}}' has no matching argument '{p} = ...'"
            ));
        }
    }
    for n in names {
        if !placeholders.iter().any(|p| p == n) && !extra.contains(&n.as_str()) {
            return Err(format!(
                "argument '{n}' is not used by any '{{{n}}}' placeholder"
            ));
        }
    }
    Ok(())
}

fn arg_names(kwargs: &[KwArg]) -> Vec<String> {
    kwargs.iter().map(|k| k.name.to_string()).collect()
}

fn arg_calls(kwargs: &[KwArg]) -> Vec<proc_macro2::TokenStream> {
    kwargs
        .iter()
        .map(|k| {
            let key = k.name.to_string();
            let expr = &k.expr;
            quote! { .arg(#key, #expr) }
        })
        .collect()
}

/// A translation message: `t!("Save")`, `t!("Welcome, {name}", name = user.name)`.
#[proc_macro]
pub fn t(input: TokenStream) -> TokenStream {
    let T { source, kwargs } = parse_macro_input!(input as T);
    let ph = match placeholders(&source.value()) {
        Ok(p) => p,
        Err(e) => return compile_error(source.span(), &e),
    };
    if let Err(e) = check(&ph, &arg_names(&kwargs), &[]) {
        return compile_error(source.span(), &e);
    }
    let args = arg_calls(&kwargs);
    quote! { ::laterite_core::i18n::Text::new(#source) #(#args)* }.into()
}

/// A context-disambiguated message (homonyms): `tp!("verb", "Open")`.
#[proc_macro]
pub fn tp(input: TokenStream) -> TokenStream {
    let Tp {
        context,
        source,
        kwargs,
    } = parse_macro_input!(input as Tp);
    let ph = match placeholders(&source.value()) {
        Ok(p) => p,
        Err(e) => return compile_error(source.span(), &e),
    };
    if let Err(e) = check(&ph, &arg_names(&kwargs), &[]) {
        return compile_error(source.span(), &e);
    }
    let args = arg_calls(&kwargs);
    quote! { ::laterite_core::i18n::Text::new(#source).with_context(#context) #(#args)* }.into()
}

/// A pluralized message: `tn!("{n} item", "{n} items", n = count)`. The `n`
/// argument is the count the plural form is chosen from.
#[proc_macro]
pub fn tn(input: TokenStream) -> TokenStream {
    let Tn { one, other, kwargs } = parse_macro_input!(input as Tn);
    let count = match kwargs.iter().find(|k| k.name == "n") {
        Some(k) => k.expr.clone(),
        None => return compile_error(one.span(), "tn! needs a count argument, 'n = ...'"),
    };
    let mut ph = match placeholders(&one.value()) {
        Ok(p) => p,
        Err(e) => return compile_error(one.span(), &e),
    };
    match placeholders(&other.value()) {
        Ok(more) => {
            for p in more {
                if !ph.iter().any(|x| x == &p) {
                    ph.push(p);
                }
            }
        }
        Err(e) => return compile_error(other.span(), &e),
    }
    if let Err(e) = check(&ph, &arg_names(&kwargs), &["n"]) {
        return compile_error(one.span(), &e);
    }
    let others: Vec<KwArg> = kwargs.into_iter().filter(|k| k.name != "n").collect();
    let others = arg_calls(&others);
    quote! {
        {
            let __n: i64 = (#count) as i64;
            ::laterite_core::i18n::Text::new(#one)
                .with_plural(#other, __n)
                .arg("n", __n)
                #(#others)*
        }
    }
    .into()
}
