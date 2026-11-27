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
`related` | none | One-to-one records a column can be read from, by name.
`title` | the resource's | The screen title.
`order_by`, `order_dir` | `id`, `desc` | The first sort; `asc` or `desc`.
`per_page` | `25` | Rows a page.
`per_page_options` | none | Page sizes offered in the list setup; empty offers no choice.
`creatable` | `false` | Offers New.
`deletable` | `false` | Rows can be selected and deleted, and a record's edit page offers Delete.
`clearable` | `false` | Offers Clear, which removes every row at once and writes an audit entry. For a log: a resource with a `form` refuses it.
`clear_permission` | none | A permission Clear asks for beside the resource's.
`protected` | none | A boolean column; a row it marks has no checkbox and no Delete.
`checkboxes` | `false` | Every row has a checkbox, whether or not the list is `deletable`.
`bulk_actions` | none | The list's own actions over ticked rows (see below).
`exportable` | `false` | Offers the export menu. See [Export](export.md).
`search` | on | The search box; keys below.
`filters` | none | Filters above the table, keyed by field name. See [Filters](filters.md).
`no_records_message` | "No records yet." | Shown when the list has no records at all.
`row_click` | `open` | Where a click on a row goes: `open` (the record's edit page), `preview` (its [preview](../forms/form-descriptor.md#preview-a-record)), `none`, or an address pattern. See below.
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

## Act on ticked rows

```yaml
list:
  bulk_actions:
    - name: publish
      label: Publish
      group: Change status
      set: { status: published }
      done: "{n} published"
    - name: retire
      label: Retire
      group: Change status
      set: { status: retired, featured: false }
      permission: acme.retire
    - name: labels
      label: Print labels
      icon: printer
      confirm: Print a label for each ticked order?
      handler: acme.labels
```

```rust
use laterite_admin::bulk::{BulkAction, BulkActionReg, BulkCx, BulkError, BulkOutcome};

struct Labels;

#[async_trait]
impl BulkAction for Labels {
    async fn run(&self, cx: &BulkCx<'_>, ids: &[String]) -> Result<BulkOutcome, BulkError> {
        let printed = print_labels(cx.db, ids).await?;
        Ok(BulkOutcome::done(printed).redirect("/orders/labels"))
    }
}

registry.add_bulk_action(BulkActionReg::new("acme.labels", Arc::new(Labels)));
```

`bulk_actions` key | Default | Description
--- | --- | ---
`name` | required | Lowercase letters, digits and hyphens, unique in the list. The action posts to `<path>/bulk/<name>`.
`label` | required | The button's text.
`icon` | none | An [icon](../reference/icons.md).
`set` | none | Form fields set on each ticked record, validated and saved as an edit of it. A resource with no form writes the columns directly. An action has `set` or `handler`.
`handler` | none | A registered bulk action's name; it receives the ticked ids.
`group` | none | Actions naming the same group share one dropdown of that label.
`confirm` | none | A question asked first.
`permission` | none | Hides the action from, and refuses it to, an operator without the grant.
`danger` | `false` | Draws the action as destructive.
`trash` | `false` | Offered in the trash view, and only there.
`done` | `Updated {n} record(s).` | The message afterwards; `{n}` is the number changed.

`BulkCx` | Description
--- | ---
`db` | The database.
`entity` | The list's table.
`actor` | Who is acting, for a write through the save pipeline.
`user` | The signed-in user and their permissions.

`BulkOutcome` | Description
--- | ---
`done(n)` | `n` records acted on.
`.refuse(message)` | One record's refusal, shown as written.
`.message(text)` | Replaces the `done` message.
`.redirect(path)` | Goes to `path` under the admin root in place of the list.

A button and a dropdown are disabled until a row is ticked; the bar shows how
many are. An unknown `handler`, a repeated `name`, a `set` field the form lacks, or an
action with both or neither of `set` and `handler` aborts the start.

## Send a row click anywhere

```yaml
list:
  row_click: "acme/orders/{id}/summary"
```

```yaml
list:
  row_click: "https://track.acme.com/parcels/{tracking_code}?order={id}"
```

Pattern | Goes to
--- | ---
`acme/orders/{id}/summary` | A path under the admin root.
`/shop/orders/{id}` | A path on the site.
`https://...` | A full address.

Placeholder | Fills in
--- | ---
`{id}` | The record's id.
`{column}` | A column on the list, as stored, URL-encoded.

A row with an empty value for a placeholder opens nothing. A record in the
trash opens nothing.

```rust
ListConfig::new("orders", t!("Orders"), columns)
    .row_click(RowClick::Url("acme/orders/{id}/summary".into()));
```

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
`sortable` | `true` | The header sorts. Empty values come last either way.
`invisible` | `false` | Hidden until the user picks it from the column chooser.
`width` | shared | A CSS width, `10%` or `120px`.
`align` | | `left`, `center`, `right`.
`permission` | none | A user without it does not see the column; it is then not queried, sorted, searched, picked or exported for them.
`searchable` | text columns | Whether the search box looks in this column.
`labels` | none | What a stored value reads as: `[{ value: login_success, label: Signed in }]`. A value not listed shows as stored.
`editable` | `false` | The cell is the form field's control, saved on change. The field must be on the resource's form.
`related` | none | The record under the list's `related` the column is read from.

## Show a column of a related record

```yaml
# descriptors/authors.yaml
list:
  related:
    profile: { entity: profiles, foreign_key: author_id }
    address: { entity: addresses, belongs_to: address_id }
  columns:
    name: {}
    city: { related: address }
    bio: { related: profile, sortable: false }
```

```rust
ListConfig::new(
    "authors",
    t!("Authors"),
    vec![
        ListColumn::new("name", t!("Name")),
        ListColumn::new("city", t!("City")).related("address"),
    ],
)
.related("address", RelatedRecord::belongs_to("addresses", "address_id"));
```

The key is the related table's column. The column sorts, searches and exports
as the list's own do. `related` takes the keys of a form's
[`related`](../forms/relations.md#edit-a-one-to-one-record-in-the-form).

A key the framework does not know is refused with its line; `lat doctor`
names it without a build.
