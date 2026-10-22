# Form Descriptor

One YAML file describes one resource: the table, where it mounts, who may reach
it, and its list and form. This page covers the resource keys and the `form`
half; the `list` half is on [List Descriptor](../lists/list-descriptor.md).

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
  columns:
    title:      { searchable: true }
    status:     { type: status_pill, width: 10% }
    created_at: { type: datetime, align: right }
form:
  title: Post
  fields:
    title:  { span: 2/3, rules: [required, { max_length: 120 }] }
    status: { span: 1/3, type: select, options: [{ value: draft, label: Draft }] }
    body:   { type: textarea, rows: 8 }
```

## Register it

```rust
registry.add_resource(laterite_admin::resource!("descriptors/posts.yaml"));
```

The path is relative to your `Cargo.toml`. The file is read at compile time: a
missing or malformed one fails the build.

Key | Description
--- | ---
`entity` | The table both screens read and write. Required.
`path` | Where the resource mounts, under your module's namespace. Required.
`title` | The navigation label, and the default title of both screens. Required.
`permission` | The permission gating it. Required once a `form` is present.
`id_field` | The primary key column. Default `id`.
`list` | The list screen. Required.
`form` | The create and edit screen. Optional.
`import` | `true`, or `{ permission: ... }`, to let records be imported into the resource. Needs `form`. See [Bulk Import](../import-export/bulk-import.md). Optional.
`relations` | Related records, keyed by name, each a section on the edit page. See [Related Records](relations.md). Needs `form`. Optional.

`entity`, `path` and `id_field` reach both screens.

## Name a field

Fields are a map. The key is the name, and they render in the order written:

```yaml
fields:
  title: {}
  published_at: { type: date }
```

A missing `label` is made from the key: `published_at` becomes "Published at".
It is still translatable, and `lat i18n extract` finds it.

## Set a field's keys

An entry carries its own keys and its type's, side by side:

```yaml
fields:
  body:   { type: textarea, rows: 8, placeholder: Write here, span: 1/2 }
  price:  { input: number, min: 0, step: 0.01, help: In the shop's currency }
  status: { type: select, options: [{ value: draft, label: Draft }], hidden: true }
```

Key | Default | Description
--- | --- | ---
`label` | from the key | The label shown.
`type` | `text` | The field type. Every type and its keys: [Form Fields](../elements/form-fields.md).
`rules` | none | Validation rules run on submit. See [Validation](validation.md).
`help` | none | A line beneath the control.
`span`, `break` | `full`, `false` | The field's place on the row. See below.
`enter` | the form's | `submit`, `off`, `next`.
`preset`, `trigger` | none | Follows or watches another field. See [Field Dependencies](field-dependencies.md).
`hidden` | `false` | Submitted but never shown: a plain hidden input carrying its value.
`translatable` | `false` | Marks the value as translatable content, for a translation plugin to manage.

A key the type does not read is refused, naming the entry and the keys it
accepts.

## Set the form's keys

```yaml
form:
  enter: off
  confirm_leave: false
  focus: off
  timestamps: true
  fields:
    title: { rules: [{ max_length: 80 }], counter: true }
    body:  { type: textarea, rows: 8, grow: false }
    token: { type: password, reveal: false }
```

Key | On | Values | Default
--- | --- | --- | ---
`title` | The form | text | The resource's `title`
`enter` | The form | `submit`, `off` | `submit`: Enter submits; Cmd/Ctrl+Enter submits from a textarea as well
`confirm_leave` | The form | `true`, `false` | `true`: leaving with changes unsaved asks first
`focus` | The form | `auto`, `off` | `auto`: a new record opens with the cursor in its first field, a refused save in the first field it refused
`timestamps` | The form | `true`, `false` | `false`; `true` stamps `created_at` and `updated_at` on write
`persist` | The form | a persister name | The built-in insert and update. See [Persisters](../database/persisters.md)
`enter` | A field | `submit`, `off`, `next` | The form's

A `max_length` rule also sets the control's `maxlength`.

## Lay out the fields

Fields lay out on a twelve-column row and wrap as it fills.

```yaml
fields:
  title:  { span: 2/3 }
  status: { span: 1/3 }
  body:   { break: true }
```

Key | Description
--- | ---
`span` | The share of the row: `full` (default), `1/2`, `1/3`, `2/3`, `1/4`, `3/4`, or `1` to `12` columns. `left`, `right`, `auto` and `half` are halves; `third` and `quarter` as named.
`break` | `true` starts a new row.

Under 1100px a quarter becomes a half; under 768px every field takes the row.

## Catch a bad key

A key the framework does not know is refused with its line:

```text
error: line 11 column 19: unknown field `searchabel`, expected one of
label, type, sortable, invisible, width, align, permission, searchable
```

Write only the keys you set; everything else takes its default.

`lat doctor` parses every `descriptors/**/*.yaml` in the application and its
plugins and names a typo without a build.

## Read a descriptor at run time

`resource!` embeds a file. For YAML you loaded yourself, call the function
behind it:

```rust
let resource = laterite_admin::descriptor::from_yaml(&yaml, "posts.yaml")?;
```
