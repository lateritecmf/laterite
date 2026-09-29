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
    status: { type: select, options: { options: [{ value: draft, label: Draft }] } }
form:
  title: Post
  fields:
    title:  { span: 2/3, rules: [required, { max_length: 120 }] }
    status: { span: 1/3, type: select, options: { options: [{ value: draft, label: Draft }] } }
    body:   { type: textarea }
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
is still translatable, and `lat i18n extract` finds it.

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
