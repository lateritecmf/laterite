# Localization

English strings in code and templates are the catalog keys. A `.po` file per
language translates them.

## Write translatable text

```rust
use laterite_core::{t, tn};

let msg = t!("Your changes were saved.");
let greeting = t!("Welcome back, {name}.").arg("name", user.name.clone());
let label = tn!("{n} item", "{n} items", count).arg("n", count);
```

Where | Call
--- | ---
Rust | `t!("...")`; `tn!("one", "many", n)` for plurals. Both build a `Text`, localized at render.
Template | `shell.t("...")`; `shell.tf(...)` with numeric placeholders.
Descriptor labels | Already `Text`, localized where they render.

A placeholder such as `{name}` appears unchanged in every translation.

## Manage catalogs

```text
crates/admin/lang/
  messages.pot     # generated template, every source string
  de.po            # a German translation
```

```sh
lat i18n extract          # regenerate every messages.pot
lat i18n update de        # create or refresh de.po from the template
lat i18n status           # per-locale coverage
lat i18n check            # fail if any messages.pot is stale; run in CI
```

A translator fills in `msgstr`:

```po
#: templates/login.html
msgid "Admin panel"
msgstr "Verwaltung"
```

## Ship a language from a module

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

A malformed catalog, or one whose placeholders differ from the source, fails
the boot.

## How a language is chosen

1. The operator's preference, from **Preferences**.
2. The browser's `Accept-Language`.
3. [`backend.locale`](../getting-started/configuration.md).
4. English.

Only a language with a loaded catalog is offered or chosen. Plural rules cover
one and other. Month and day names follow a locale that carries a territory,
`de_DE`.

## Preview coverage

Set the locale to `xx`, or send `Accept-Language: xx`: every translated
string renders bracketed and accented, `[Ṡäṽé]`. Plain text is a string that
bypassed translation.
