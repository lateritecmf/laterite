# List Columns

Every built-in column type. The keys every column takes (`label`, `width`,
`labels`, ...) are on [List Descriptor](../lists/list-descriptor.md), and a type
of your own is [Writing a Column Type](../lists/column-types.md).

## text

```yaml
columns:
  title: { searchable: true }
```

The value as stored, or its entry under `labels`. The type when none is
written; the search box looks in it.

## datetime

```yaml
columns:
  created_at: { type: datetime, align: right }
```

A date and time in the display timezone.

## date

```yaml
columns:
  published_on: { type: date }
```

A date in the display timezone.

## time

```yaml
columns:
  opens_at: { type: time }
```

A time of day in the display timezone.

## boolean

```yaml
columns:
  is_live: { type: boolean, width: 80px }
```

Value | Shown as
--- | ---
`1`, `true` | Yes
`0`, `false`, empty | No

## status_pill

```yaml
columns:
  status: { type: status_pill, labels: [{ value: draft, label: Draft }, { value: live, label: Live }] }
```

A pill coloured by the value, lowercased with anything but letters and digits
made a hyphen, and reading as its entry under `labels`.

## link

```yaml
columns:
  website:   { type: link, new_tab: true }
  instagram: { type: link, icon: instagram, text: "@{value|handle}", new_tab: true }
  order:     { type: link, url: "acme/orders/{id}/edit", text: "Open {code}" }
```

Key | Default | Description
--- | --- | ---
`url` | the value | The address, with `{value}` for the cell's value, `{id}` for the record's and `{column}` for another column on the list. A value filled in is URL-encoded. An address with no scheme and no leading `/` is under the admin.
`text` | the value | What the link reads, with the same placeholders. `""` shows the icon alone.

Placeholder | Fills in
--- | ---
`{value}`, `{id}`, `{column}` | The value as stored
`{value\|handle}` | The last part of a link's path, without `@`: `band` from `https://social.acme/@band`
`{value\|host}` | A link's domain, without `www.`: `acme.com`
`icon` | none | An icon from the [icon set](../reference/icons.md), before the text.
`new_tab` | `false` | Opens the address in a new tab.
`format` | the field's | Shows a stored address as its prefixed name. See [Field Formats](../forms/field-formats.md).

Case | Shown as
--- | ---
`http`, `https`, `mailto`, `tel` or a `/` path | A link
Any other address (`javascript:`, `data:`) | Text
A placeholder whose column is not on the list, or is empty | Text
An empty value with no `url` | Nothing
