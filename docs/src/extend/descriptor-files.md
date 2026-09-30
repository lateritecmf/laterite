# Descriptor Files

One YAML file describes one resource: the table, where it mounts, who may reach
it, and its list and form.

```yaml
# descriptors/posts.yaml
entity: posts
path: /posts
title: Posts
permission: acme.blog.manage_posts
list:
  creatable: true
  deletable: true
  order_by: created_at
  order_dir: desc
  no_records_message: No posts yet.
  search: { prompt: Search posts }
  columns:
    title:      { searchable: true }
    status:     { type: status_pill, width: 10% }
    created_at: { type: datetime, align: right }
  filters:
    status: { type: select, options: [{ value: draft, label: Draft }] }
form:
  title: Post
  fields:
    title:  { span: 2/3, rules: [required, { max_length: 120 }] }
    status: { span: 1/3, type: select, options: [{ value: draft, label: Draft }] }
    body:   { type: textarea, rows: 8 }
```

Register it from your module:

```rust
registry.add_resource(laterite_admin::resource!("descriptors/posts.yaml"));
```

The path is relative to your `Cargo.toml`. The file is read at compile time: a
missing or malformed one fails the build.

## Resource keys

Key | Description
--- | ---
`entity` | The table both screens read and write. Required.
`path` | Where the resource mounts, under your module's namespace. Required.
`title` | The navigation label, and the default title of both screens. Required.
`permission` | The permission gating it. Required once a `form` is present.
`id_field` | The primary key column. Default `id`.
`list` | The list screen. Required.
`form` | The create and edit screen. Optional.

`entity`, `path` and `id_field` reach both screens.

## Naming

Columns, fields and filters are maps. The key is the name, and they render in
the order written:

```yaml
columns:
  title: {}
  created_at: { type: datetime }
```

A missing `label` is made from the key: `created_at` becomes "Created at". It
is still translatable, and `lat i18n extract` finds it, as it finds the `label`
of every entry under a column's `labels`.

## Type keys

An entry carries its own keys and its type's, side by side:

```yaml
fields:
  body:   { type: textarea, rows: 8, placeholder: Write here }
  price:  { input: number, min: 0, step: 0.01 }
  status: { type: select, options: [{ value: draft, label: Draft }] }
filters:
  status: { type: select, options: [{ value: draft, label: Draft }] }
```

Type | Keys
--- | ---
`text` | `input` (`text`, `email`, `tel`, `number`, `url`), `placeholder`, `counter`. A `number` input adds `min`, `max`, `step`; a `url` input adds `copy`.
`textarea` | `rows`, `placeholder`, `grow`, `counter`
`select` | `options`: a list of `{ value, label }`; `search`
`radio` | `options`
`checklist` | `options`, `select_all`, `search`, `expand`. See [Checklist](#checklist).
`reference` | `source`
`repeater` | `fields`, `min_items`, `max_items`, `display`, `summary_field`, `reorder`, `duplicate`
`password` | `reveal`
`switch`, `date` | None
A `select` filter | `options`

A key the type does not read is refused, naming the entry and the keys it
accepts.

## Checklist

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

## Form keys

```yaml
form:
  enter: off
  confirm_leave: false
  focus: off
  fields:
    title: { rules: [{ max_length: 80 }], counter: true }
    body:  { type: textarea, rows: 8, grow: false }
    token: { type: password, reveal: false }
```

Key | On | Values | Default
--- | --- | --- | ---
`enter` | The form | `submit`, `off` | `submit`
`confirm_leave` | The form | `true`, `false` | `true`: leaving with changes unsaved asks first
`focus` | The form | `auto`, `off` | `auto`: a new record opens with the cursor in its first field, a refused save in the first field it refused
`enter` | A field | `submit`, `off`, `next` | The form's
`counter` | `text`, `textarea` | `auto`, `true`, `false` | `auto`: with a `max_length` rule, shown once four fifths of it are used
`grow` | `textarea` | `true`, `false` | `true`: as tall as its text, from `rows` up to most of the window
`reveal` | `password` | `true`, `false` | `true`: a button shows what was typed
`search` | `select` | `auto`, `true`, `false` | `auto`: past 10 choices, the dropdown is searched. An option with an empty value is what clearing chooses.
`display` | `repeater` | `auto`, `inline`, `list` | `auto`: past 3 fields a row, rows collapse to the line naming them
`reorder` | `repeater` | `true`, `false` | `true`: each row moves up and down
`duplicate` | `repeater` | `true`, `false` | `true`: each row copies into a new one beneath it

A `max_length` rule also sets the control's `maxlength`.

## Dependencies

```yaml
fields:
  title: {}
  slug:  { preset: title }
  path:  { preset: { field: title, type: url } }
  is_delayed: { type: switch, label: Send later }
  send_at:
    type: date
    trigger: { action: show, field: is_delayed, condition: checked }
  format: { type: select, options: [{ value: csv }, { value: xml }] }
  delimiter:
    trigger: { action: show|empty, field: format, condition: "value[csv]" }
  topics: { type: checklist, options: [{ value: news }, { value: sport }] }
  region:
    trigger: { action: enable, field: "topics[]", condition: "value[sport]" }
```

Key | Value
--- | ---
`preset` | The field followed, or `{ field, type }`. Follows it as it is typed, until this field is edited; emptying this field hands it back.
`preset.type` | `slug` (default): `hello-world`. `url`: `/hello-world`. `file`: spaces to hyphens. `exact`: as typed.
`trigger.action` | `show`, `hide`, `enable`, `disable`, `empty`, `fill[value]`, several joined by `\|`. A hidden field is left out of the submission.
`trigger.field` | The field watched. `name[]` watches every ticked value of a checklist.
`trigger.condition` | `checked`, `unchecked`, `value[x]`, `value[x][y]` (either), `value[]` (empty), `value[*]` (anything), `value[csv*]` (a prefix).

A field that follows or watches one the form does not have stops the boot. An
action or condition the page does not know is refused when the file is read.

## Layout

Fields lay out on a twelve-column row and wrap as it fills.

Key | Description
--- | ---
`span` | The share of the row: `full` (default), `1/2`, `1/3`, `2/3`, `1/4`, `3/4`, or `1` to `12` columns. `left`, `right`, `auto` and `half` are halves; `third` and `quarter` as named.
`break` | `true` starts a new row.

Under 1100px a quarter becomes a half; under 768px every field takes the row.

## Errors

A key the framework does not know is refused with its line:

```text
error: line 11 column 19: unknown field `searchabel`, expected one of
label, type, sortable, invisible, width, align, permission, searchable
```

Write only the keys you set; everything else takes its default.

`lat doctor` parses every `descriptors/**/*.yaml` in the application and its plugins,
so a typo is named without a build.

## Read a descriptor at run time

`resource!` embeds a file. For YAML you loaded yourself, call the function
behind it:

```rust
let resource = laterite_admin::descriptor::from_yaml(&yaml, "posts.yaml")?;
```
