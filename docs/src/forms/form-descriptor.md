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
`source` | Keeps the records as files in a folder of the project. See [Records in Files](../database/records-in-files.md). Needs `form`. Optional.

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
`format` | none | The shapes accepted, the one stored, and how a `link` column shows it. See [Field Formats](field-formats.md).
`hidden` | `false` | Submitted but never shown: a plain hidden input carrying its value.
`transient` | `false` | Shown and submitted with no column behind it: not loaded, not written by the built-in writer; the form's `persist` handler reads it (a password field).
`translatable` | `false` | Marks the value as translatable content, for a translation plugin to manage.
`context` | every page | The pages the field is on: `create`, `update`, `preview`, one or a list. See below.
`default` | none | What a new record's form opens with: text, a number, `true` or `false`, or a list for a field holding several.
`default_from` | none | Another field whose `default` this one opens with.
`read_only` | `false` | Shown with its value, not editable. A save keeps what is stored; a new record takes the field's `default`.
`disabled` | `false` | Shown, not editable, and left out of every save.
`permission` | none | A permission the user must hold to see the field or write it. An editable list cell of the field follows the same rule.
`depends_on` | none | A field, or a list of fields: when one changes, this field is drawn again. See [Field Dependencies](field-dependencies.md#options-from-a-source).

A key the type does not read is refused, naming the entry and the keys it
accepts.

## Put fields on tabs

```yaml
form:
  fields:
    title: {}
  tabs:
    default_tab: Details
    active_tab: Pricing
    icons: { Details: file-text, Pricing: tag }
    fields:
      body:  { type: textarea }
      price: { tab: Pricing, input: number }
      stock: { tab: Pricing, input: number }
  secondary_tabs:
    fields:
      notes: { tab: Notes, type: textarea }
```

Part | Holds
--- | ---
`fields` | The fields above the tabs. One naming a `tab` joins the tabs.
`tabs.fields` | The fields on the tabs, each on the `tab` it names.
`secondary_tabs.fields` | A second set of tabs, under the first.

Key of `tabs` and `secondary_tabs` | Default | Description
--- | --- | ---
`default_tab` | `Misc` | The tab for a field in the set that names none.
`active_tab` | the first | The tab open on load: its label, or its place from 1.
`icons` | none | An icon for a tab, by the tab's label.
`linkable` | `true` | A tab opens by the address's fragment, and writes it.
`stretch` | `false` | The panes fill the height left on the page.
`css_class` | none | A class on the set.
`pane_css_class` | none | A class on every pane, or a map of tab label to class.
`identifiers` | from the label | An HTML id for a tab, by its label.

Tabs show in the order their fields first name them. A refused save opens
the first tab holding a refused field.

```js
lat.get(document.querySelector('[data-lat-widget="formtabs"]')).open('tab-pricing');
lat.on('formtabs:opened', function (detail) { console.log(detail.tab); });
```

```rust
FormField::text("price", t!("Price")).tab(t!("Pricing"));
```

## Keep a field to some pages

```yaml
fields:
  code:  {}
  token: { context: create }
  notes: { type: textarea, context: [update, preview] }
  status@create: { type: select, options: [{ value: draft, label: Draft }] }
  status@update: { type: select, options: [{ value: draft, label: Draft }, { value: live, label: Live }] }
```

Form | Meaning
--- | ---
`context: update` | On that page alone.
`context: [create, update]` | On each page listed.
`name@update` | The same, written on the key. One column may have an entry a page.

Page | Route
--- | ---
`create` | `{path}/new`
`update` | `{path}/{id}/edit`
`preview` | `{path}/{id}/preview`

A field off a page is not shown, checked or saved there. An import and an
ability write through every field.

```rust
FormField::text("token", t!("Token")).context([FormContext::Create]);
```

## Set the form's keys

```yaml
form:
  enter: off
  confirm_leave: false
  focus: off
  timestamps: true
  footprints: true
  slugs: { slug: title }
  encrypt: [api_key]
  actions:
    - { label: Reset to default, action: "acme/templates/{id}/reset", confirm: Reset this template? }
    - { label: Download PDF, action: "acme/templates/{id}/pdf", method: get }
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
`footprints` | The form | `true`, `false` | `false`; `true` stamps `created_user_id` and `updated_user_id`. See [Model listeners](../database/model-listeners.md#record-who-changed-a-row)
`slugs` | The form | a map of slug column to a source column or a list | None. See [Model listeners](../database/model-listeners.md#generate-a-slug)
`encrypt` | The form | a list of columns | None. See [Model listeners](../database/model-listeners.md#seal-a-column)
`related` | The form | a map of one-to-one records | None. See [Related Records](relations.md#edit-a-one-to-one-record-in-the-form)
`related` | A field | a name under the form's `related` | None: the field is stored on that record
`persist` | The form | a persister name | The built-in insert and update. See [Persisters](../database/persisters.md)
`actions` | The form | a list of `label`, `action` (a path under the admin root, `{id}` for the record), `method` (`post` or `get`), `confirm`, `permission` (hides the button without the grant) | None. Shown beside Save on an existing record; `post` submits the form to `action`, `get` links to it
`panels` | The form | a list of `src`, `title`, `permission`, as a [preview's panels](#preview-a-record) | None. Drawn under an existing record's form, each fetched from `src` once the page is open
`linked_record` | The form | `morph` (the name `n` of the columns `n_type` and `n_id`), `label`, `empty` | None. Links the owning record above an edit form, named by its resource's title and its list's first column; `empty` ("Not linked") shows when the columns are empty
`enter` | A field | `submit`, `off`, `next` | The form's

A `max_length` rule also sets the control's `maxlength`.

## Preview a record

```yaml
# descriptors/orders.yaml
preview:
  title: Order
  assets: [chart.css]
  panels:
    - { title: Payments, src: "acme/orders/{id}/payments", permission: acme.payments }
list:
  row_click: preview
```

`preview: true` takes every default.

```yaml
preview:
  fields:
    code:   { label: Reference }
    total:  { input: number }
  relations: false
```

Key | Default | Description
--- | --- | ---
`title` | the form's | The page's title.
`assets` | none | Asset keys the panels need on the page.
`panels` | none | Parts drawn under the values, in order.
`fields` | the form's on `preview` | The preview's own fields, written as the form's are.
`relations` | `true` | The record's related lists show under the values, to read.

Panel key | Description
--- | ---
`src` | A path under the admin root answering an HTML fragment, `{id}` for the record's id. Fetched once the page is open.
`title` | The heading over the panel.
`permission` | A user without it does not get the panel.

Route | Shows
--- | ---
`{path}/{id}/preview` | The form's fields with their values, none editable, an Edit button, then the panels.

A previewed resource's list shows a Preview link on each row, before Edit.

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
