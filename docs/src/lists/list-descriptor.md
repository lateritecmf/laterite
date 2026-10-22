# List Descriptor

The `list` half of a resource file: its columns, search, ordering, paging and
the capabilities it offers. The resource keys around it are on
[Form Descriptor](../forms/form-descriptor.md).

```yaml
# descriptors/posts.yaml
entity: posts
path: /posts
title: Posts
permission: acme.blog.manage_posts
list:
  creatable: true
  deletable: true
  exportable: true
  order_by: created_at
  order_dir: desc
  per_page: 25
  per_page_options: [25, 50, 100]
  no_records_message: No posts yet.
  search: { prompt: Search posts }
  columns:
    title:      { searchable: true }
    status:     { type: status_pill, width: 10%, labels: [{ value: draft, label: Draft }] }
    created_at: { type: datetime, align: right }
  filters:
    status: { type: select, options: [{ value: draft, label: Draft }] }
  toolbar:
    - { label: Archive, href: /posts/archive, icon: archive, primary: false }
```

## Set the list's keys

Key | Default | Description
--- | --- | ---
`columns` | | The columns, keyed by field name, in order. Required.
`title` | the resource's | The screen title.
`order_by`, `order_dir` | `id`, `desc` | The first sort; `asc` or `desc`.
`per_page` | `25` | Rows a page.
`per_page_options` | none | Page sizes offered in the list setup; empty offers no choice.
`creatable` | `false` | Offers New.
`deletable` | `false` | Rows can be selected and deleted.
`exportable` | `false` | Offers the export menu. See [Export](export.md).
`search` | on | The search box; keys below.
`filters` | none | Filters above the table, keyed by field name. See [Filters](filters.md).
`no_records_message` | "No records yet." | Shown when the list has no records at all.
`row_click` | `open` | `open`: a row opens its record. `none`: the Edit link alone does.
`remember` | `true` | The sort, search, filters and page a user last used come back when they return.
`reorder` | off | `true` orders rows by `sort_order`; `{ column: position }` names the column. See [Trees and Reordering](structures.md).
`tree` | off | `true` nests rows on `parent_id`; `{ parent_key, source }` names the column and a picker source for Move to. See [Trees and Reordering](structures.md).
`toolbar` | none | Extra buttons beside New; keys below.

`import` is a resource key beside `list`: see [Bulk Import](../import-export/bulk-import.md).

Key | Default | Description
--- | --- | ---
`search.enabled` | `true` | `false` removes the box.
`search.prompt` | "Search" | The placeholder.
`search.on_enter` | `false` | Ask on Enter only.
`toolbar[].label` | | The button's text. Required.
`toolbar[].href` | | A path under the admin root, or an absolute URL. Required.
`toolbar[].icon` | none | An icon name from the [shared set](../reference/icons.md).
`toolbar[].permission` | none | Hidden from a user who lacks it.
`toolbar[].primary` | | `true` renders it as the prominent action. Required.

## Name a column

Columns are a map. The key is the name, and they render in the order written:

```yaml
columns:
  title: {}
  created_at: { type: datetime }
```

A missing `label` is made from the key: `created_at` becomes "Created at". It
is still translatable, and `lat i18n extract` finds it, as it finds the `label`
of every entry under a column's `labels`.

## Set a column's keys

Key | Default | Description
--- | --- | ---
`label` | from the key | The header.
`type` | `text` | The column type. Every type: [List Columns](../elements/list-columns.md).
`sortable` | `true` | The header sorts.
`invisible` | `false` | Hidden until the user picks it from the column chooser.
`width` | shared | A CSS width, `10%` or `120px`.
`align` | | `left`, `center`, `right`.
`permission` | none | A user without it does not see the column; it is then not queried, sorted, searched, picked or exported for them.
`searchable` | text columns | Whether the search box looks in this column.
`labels` | none | What a stored value reads as: `[{ value: login_success, label: Signed in }]`. A value not listed shows as stored.
`editable` | `false` | The cell is the form field's control, saved on change. The field must be on the resource's form.

A key the framework does not know is refused with its line; `lat doctor`
names it without a build.
