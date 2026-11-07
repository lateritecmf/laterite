//! Translation-string macros.
//!
//! `t!`, `tn!`, `tp!` build a `laterite_core::i18n::Text` at the call site. The
//! source is a string literal (so extraction can collect it), its `{name}`
//! placeholders are checked against the named arguments at compile time, and
//! positional `{}` or format specs are rejected. For a non-literal source, use
//! `Text::dynamic`.

#![warn(missing_docs)]

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

/// Implements `laterite_core::model::Model` for a struct whose named fields
/// are its table's columns.
///
/// ```ignore
/// #[derive(laterite_core::Model)]
/// #[model(table = "posts")]
/// struct Post {
///     id: i64,
///     title: String,
///     #[model(column = "published_at")]
///     published: Option<DateTime<Utc>>,
/// }
/// ```
///
/// `table` is required; `id = "..."` names the id field when it is not `id`.
/// Each field's type implements `FromColumn` and `ToColumn`.
#[proc_macro_derive(Model, attributes(model))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match model_impl(&input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn model_impl(input: &syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let mut table: Option<String> = None;
    let mut id_field = "id".to_string();
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("model")) {
        attr.parse_nested_meta(|meta| {
            let value: syn::LitStr = meta.value()?.parse()?;
            if meta.path.is_ident("table") {
                table = Some(value.value());
            } else if meta.path.is_ident("id") {
                id_field = value.value();
            } else {
                return Err(meta.error("expected `table` or `id`"));
            }
            Ok(())
        })?;
    }
    let table = table.ok_or_else(|| {
        syn::Error::new_spanned(name, "#[derive(Model)] needs #[model(table = \"...\")]")
    })?;
    let syn::Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            name,
            "#[derive(Model)] needs a struct",
        ));
    };
    let syn::Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            name,
            "#[derive(Model)] needs named fields",
        ));
    };
    let mut idents = Vec::new();
    let mut columns = Vec::new();
    for field in &fields.named {
        let ident = field.ident.clone().expect("named field");
        let mut column = ident.to_string();
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("model")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("column") {
                    let value: syn::LitStr = meta.value()?.parse()?;
                    column = value.value();
                    Ok(())
                } else {
                    Err(meta.error("expected `column`"))
                }
            })?;
        }
        idents.push(ident);
        columns.push(column);
    }
    let Some(id_at) = idents.iter().position(|i| *i == id_field) else {
        return Err(syn::Error::new_spanned(
            name,
            format!("#[derive(Model)] needs an `{id_field}: i64` field"),
        ));
    };
    let id_ident = &idents[id_at];
    let id_column = &columns[id_at];
    // The id first in COLUMNS, as the trait promises.
    let mut ordered: Vec<&String> = vec![id_column];
    ordered.extend(columns.iter().filter(|c| *c != id_column));
    let reads = idents.iter().zip(&columns).map(|(ident, column)| {
        quote! { #ident: ::laterite_core::model::FromColumn::from_column(row, #column)? }
    });
    let writes = idents
        .iter()
        .zip(&columns)
        .filter(|(ident, _)| *ident != id_ident)
        .map(|(ident, column)| {
            quote! { rec.set(#column, ::laterite_core::model::ToColumn::to_column(&self.#ident)); }
        });
    Ok(quote! {
        impl ::laterite_core::model::Model for #name {
            const TABLE: &'static str = #table;
            const ID: &'static str = #id_column;
            const COLUMNS: &'static [&'static str] = &[#(#ordered),*];

            fn from_row(
                row: &::laterite_core::model::AnyRow,
            ) -> ::std::result::Result<Self, ::laterite_core::model::Error> {
                ::std::result::Result::Ok(Self { #(#reads),* })
            }

            fn to_record(&self) -> ::laterite_core::Record {
                let mut rec = ::laterite_core::Record::new(#table);
                #(#writes)*
                rec
            }

            fn id(&self) -> i64 {
                self.#id_ident
            }
        }
    })
}
