# Localization

The admin panel is fully translatable. English is both the source language and
the catalog key: you write plain English strings in code and templates, and a
translation catalog maps each one to another language. There are no string IDs
to invent or keep in sync.

English is the source language, and a fresh install starts with English only.
Other languages are translation catalogs, added as data, never hardcoded in the
framework's code. The admin's own strings get community translations shipped
with the framework; an application adds catalogs for its own strings. The
languages available are exactly the catalogs that are loaded, so there is no
fixed list of languages to maintain.

## Writing translatable text

User-facing strings go through the translation layer rather than being emitted
raw:

- In Rust, build a message with the `t!` macro (and `tn!` for plurals). It
  produces a `Text` value that is localized later, when the request's language
  is known.
- In a template, call `shell.t("...")` for a plain string, or `shell.tf(...)`
  for one with numeric placeholders.
- Descriptor labels (nav, list columns, form fields, settings, permissions) are
  `Text`, localized where they render.

```rust
use laterite_core::t;

let msg = t!("Your changes were saved.");
let greeting = t!("Welcome back, {name}.").arg("name", user.name.clone());
```

The English text is the key, so a placeholder like `{name}` must appear
unchanged in every translation.

## The catalog workflow

Each crate keeps its catalogs under `lang/`: a generated `messages.pot`
template and one `<locale>.po` per translated language.

```text
crates/admin/lang/
  messages.pot     # generated template, every source string
  de.po            # a German translation
  fr.po            # a French translation
```

Four commands manage them:

```sh
lat i18n extract          # regenerate every messages.pot from the source
lat i18n update de        # create/refresh de.po from the template
lat i18n status           # report per-locale translation coverage
lat i18n check            # fail if any messages.pot is stale (run in CI)
```

`extract` scans the code and descriptors and writes a deterministic
`messages.pot`. It is generated, never hand-edited. `update <locale>` merges the
template into `<locale>.po`, keeping existing translations and adding empty
entries for new strings. A translator then fills in the `msgstr` lines:

```po
#: templates/login.html
msgid "Admin panel"
msgstr "Verwaltung"
```

`check` runs in the verification loop, so a catalog can never drift from the
code: change a string, and CI fails until `extract` has been run and the
`.pot` committed.

## Shipping a language from a module

A module contributes its translations by overriding `catalogs`, returning
`(locale, po_text)` pairs baked into the binary with `include_str!`:

```rust
impl Module for AcmeModule {
    fn catalogs(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("de", include_str!("../lang/de.po")),
            ("fr", include_str!("../lang/fr.po")),
        ]
    }
}
```

The framework's own admin translations ship this same way, from the admin
crate's `lang/` folder, so the built-in panel gains languages as translations
are contributed.

The boot loader merges every module's catalogs into one store. A malformed
catalog (bad PO, or a translation whose placeholders do not match its source)
fails boot, the same way a bad descriptor does, so a broken translation never
ships silently.

## How a language is chosen

Each request resolves a language in order:

1. The signed-in user's own preference, set from **Preferences**.
2. The browser's `Accept-Language` header.
3. The deployment default from [`backend.locale`](../getting-started/configuration.md).
4. English, always reachable as the source.

A candidate is used only if a catalog for it is loaded (English aside), so the
language actually rendered always matches the page's declared `lang`. The
Preferences picker offers exactly English plus every language with a loaded
catalog. Ship a `de.po` and German appears in the picker and the resolution
chain with no other change.

## Plurals

A count-dependent message carries both forms and the number:

```rust
use laterite_core::tn;

let label = tn!("{n} item", "{n} items", count).arg("n", count);
```

The catalog stores the forms per language, and the correct one is chosen by the
language's plural rule. The plural rules currently cover the one/other
distinction that most languages use. A language that distinguishes more forms
(few, many, zero, two) is not yet fully supported.

## Dates

Dates render their month and day names in the request's language automatically,
using the same locale resolution. This needs a territory-bearing locale tag (for
example `de_DE`, not a bare `de`); a language with no territory falls back to
neutral English month names. See [Dates and
Timezones](../getting-started/dates-and-timezones.md).

## Previewing coverage

Set the deployment locale to the pseudo-locale `xx` (or send
`Accept-Language: xx`) to render every translatable string accented and
bracketed, for example `[Ṡäṽé]`. Anything that shows as plain, unbracketed text
is a string that was not routed through the translation layer. The pseudo-locale
never appears in the language picker.
