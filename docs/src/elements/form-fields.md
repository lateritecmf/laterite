# Form Fields

Every built-in form field type, with the keys it reads. The keys every field
takes (`label`, `span`, `rules`, ...) are on [Form
Descriptor](../forms/form-descriptor.md); a type of your own is [Writing a
Field Type](../forms/field-types.md). A key a type does not read is refused,
naming the entry and the keys it accepts.

## text

```yaml
fields:
  email: { type: text, input: email, placeholder: you@acme.test }
  price: { input: number, min: 0, step: 0.01 }
  site:  { input: url, copy: true }
```

Key | Default | Description
--- | --- | ---
`input` | `text` | `text`, `email`, `tel`, `number`, `url`, `color`.
`placeholder` | none | The placeholder.
`counter` | `auto` | `auto`, `true`, `false`. `auto`: with a `max_length` rule, shown once four fifths of it are used.
`min`, `max`, `step` | none | A `number` input's bounds and step.
`copy` | `false` | A `url` input: a button copying the value.

`text` is the type when none is written. An `email` input adds the email rule.

## textarea

```yaml
fields:
  body: { type: textarea, rows: 8, placeholder: Write here, grow: false }
```

Key | Default | Description
--- | --- | ---
`rows` | | The height, in lines.
`placeholder` | none | The placeholder.
`grow` | `true` | As tall as its text, from `rows` up to most of the window.
`counter` | `auto` | As for `text`.

## richeditor

```yaml
fields:
  body: { type: richeditor, rows: 12, toolbar: [format, bold, italic, "|", link, image] }
```

Key | Default | Description
--- | --- | ---
`rows` | | The height, in lines.
`placeholder` | none | The placeholder.
`toolbar` | every entry, in this order | A list from `format`, `bold`, `italic`, `underline`, `strike`, `bullet_list`, `ordered_list`, `link`, `image`, `table`, `rule`, `undo`, `redo`, `|`.

Stores HTML kept to that set; an image from the library is linked to the
record.

## markdown

```yaml
fields:
  body: { type: markdown, rows: 12, preview: true }
```

Key | Default | Description
--- | --- | ---
`rows` | | The height, in lines.
`placeholder` | none | The placeholder.
`preview` | `true` | A Preview tab rendered by the server.

Stores the markdown as written; the site renders it with the
[`markdown` filter](../markup/filters.md).

## codeeditor

```yaml
fields:
  styles: { type: codeeditor, language: css, rows: 20, wrap: true }
```

Key | Default | Description
--- | --- | ---
`language` | `text` | `text`, `html`, `css`, `javascript`, `json`, `yaml`, `markdown`, `jinja`.
`rows` | | The height, in lines.
`line_numbers` | `true` | Numbers in the gutter.
`wrap` | `false` | `false`: long lines scroll.
`placeholder` | none | The placeholder.

## select

```yaml
fields:
  status: { type: select, options: [{ value: "", label: Any }, { value: draft, label: Draft }], search: auto }
```

Key | Default | Description
--- | --- | ---
`options` | | A list of `{ value, label }`. Required.
`search` | `auto` | `auto`, `true`, `false`. `auto`: past 10 choices, the dropdown is searched. An option with an empty value is what clearing chooses.

## radio

```yaml
fields:
  size: { type: radio, options: [{ value: s, label: Small }, { value: l, label: Large }] }
```

Key | Default | Description
--- | --- | ---
`options` | | A list of `{ value, label }`. Required.

## checklist

Boxes in groups. The column holds the ticked values as a JSON array of text.

```yaml
fields:
  topics:
    type: checklist
    rules: [required]        # a box must be ticked
    options:
      - { value: news, label: News }
      - group: Sport
        options:
          - { value: football, label: Football, note: Every league }
          - { value: cricket, label: Cricket }
    select_all: auto
    search: auto
    expand: auto
```

Key | Values | Default
--- | --- | ---
`options` | A list of `{ value, label, note }` and `{ group, options }`, nested to any depth. | None
`select_all` | `auto`, `true`, `false` | `auto`: past 10 choices
`search` | `auto`, `true`, `false` | `auto`: past 20 choices
`expand` | `auto`, `all`, `none` | `auto`: past 10 choices in more than one group, a group with every box ticked or none starts closed and a group partly ticked starts open

A value listed twice stops the boot, naming it.

## switch

```yaml
fields:
  is_live: { type: switch, label: Published }
```

No keys. A yes or no over a boolean column.

## date

```yaml
fields:
  published_at: { type: date }
```

No keys. The value must be a date.

## password

```yaml
fields:
  token: { type: password, reveal: false }
```

Key | Default | Description
--- | --- | ---
`reveal` | `true` | A button shows what was typed.

## sensitive

```yaml
fields:
  api_token: { type: sensitive }
```

A secret sealed under the [application key](../services/crypt.md) before it is stored and shown as a mask. Reveal shows the stored value and writes `<entity>.reveal` to the audit log; a blank or the mask submitted back leaves the value as it is. The column is text; `lat crypt rotate` re-seals it after a key rotation. No keys of its own.

## id

```yaml
fields:
  post_id: { type: id }
```

No keys. A record id held as a number, as a hidden input; a relation's foreign
key is one.

## reference

```yaml
fields:
  author_id: { type: reference, source: acme.authors }
```

Key | Default | Description
--- | --- | ---
`source` | | A registered picker source. Required.

The column holds the chosen id as a number.

## repeater

```yaml
fields:
  links:
    type: repeater
    min_items: 1
    max_items: 10
    summary_field: label
    fields:
      label: {}
      url:   { input: url }
```

Key | Default | Description
--- | --- | ---
`fields` | | The row's fields, a map like the form's. Required.
`min_items`, `max_items` | none | How many rows the field needs and allows.
`display` | `auto` | `auto`, `inline`, `list`. `auto`: past 3 fields a row, rows collapse to the line naming them.
`summary_field` | the first field | Which field names a collapsed row.
`reorder` | `true` | Each row moves up and down.
`duplicate` | `true` | Each row copies into a new one beneath it.

## media

```yaml
fields:
  cover_id: { type: media, mode: image }
  gallery:  { type: media, max: 6 }
```

Key | Default | Description
--- | --- | ---
`mode` | `file` | `image` offers images alone and shows thumbnails.
`max` | `1` | How many files the field holds; `0` is no limit.

The columns it writes and the links it keeps: [Media Field](../media/field.md).
