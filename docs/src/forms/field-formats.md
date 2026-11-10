# Field Formats

A format says what a value looks like. The form accepts the shapes people
type and stores one; a `link` column on the same field shows it short and
linked.

```yaml
form:
  fields:
    instagram_url:
      label: Instagram
      format: { base: "https://www.instagram.com/", prefix: "@", pattern: "[A-Za-z0-9._]{1,30}", lowercase: true }
list:
  columns:
    instagram_url: { type: link, new_tab: true }
```

```rust
FormField::text("instagram_url", t!("Instagram")).format(
    FieldFormat::new("https://www.instagram.com/").prefix("@").pattern("[A-Za-z0-9._]{1,30}").lowercase(),
)
```

Key | Default | Description
--- | --- | ---
`base` | required | The address every value sits under, ending in `/`. A path is allowed: `https://www.linkedin.com/in/`.
`prefix` | none | Shown before the name, and accepted before it when typed.
`pattern` | any | A regular expression the whole name must match.
`lowercase` | `false` | Stores the name lowercased.

## What the form accepts

Typed | Stored
--- | ---
`https://www.instagram.com/acme/?igsh=1` | `https://www.instagram.com/acme`
`instagram.com/acme` | `https://www.instagram.com/acme`
`@acme` | `https://www.instagram.com/acme`
`acme` | `https://www.instagram.com/acme`
Empty | Empty; add `required` to refuse it
Another site, a path or a name `pattern` refuses | Refused on the field

## What the list shows

Stored | Shown
--- | ---
`https://www.instagram.com/acme` | `@acme`, linked
A value saved before the format | As stored

A `link` column reads its field's format; `format` on the column itself
overrides it, for a list with no form.

```yaml
list:
  columns:
    profile: { type: link, format: { base: "https://www.tiktok.com/", prefix: "@" } }
```
