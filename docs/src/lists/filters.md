# Filters

A filter narrows the list to rows matching a value the user picks. Only a
value a declared filter accepts reaches the query.

```yaml
list:
  filters:
    status:     { type: select, options: [{ value: draft, label: Draft }, { value: live, label: Live }], default_value: live }
    featured:   { type: boolean }
    author:     { type: text }
    views:      { type: number }
    created_at: { type: date, permission: acme.blog.audit }
```

Key | Default | Description
--- | --- | ---
`label` | from the key | The control's label.
`type` | `text` | `boolean`, `select`, `text`, `number` or `date`.
`options` | | A `select` filter's choices, a list of `{ value, label }`. Required for `select`.
`default_value` | none | Applied when the request names no value for this filter: the screen opens already narrowed. Clearing it is still "no filter".
`permission` | none | A user without it does not see the filter, and its default is not applied.

A key written bare (`author:`) is a text filter with every default.

## Narrow by a range or a related record

```yaml
list:
  related:
    author: { entity: authors, belongs_to: author_id }
  filters:
    views_min:   { label: Views at least, type: number, column: views, match: at_least }
    views_max:   { label: Views at most, type: number, column: views, match: at_most }
    author_city: { label: Author's city, type: text, column: city, related: author }
```

Key | Default | Description
--- | --- | ---
`column` | the filter's key | The column compared, so two filters narrow one column.
`match` | `equals` | On a `number` filter: `equals`, `at_least` or `at_most`.
`related` | none | A record under the list's `related`; the filter compares its column.

## Pick a scope type

Type | The value reaches the query as
--- | ---
`boolean` | `1` or `0`: rows where the column is true or false.
`select` | A declared `value`, matched exactly; any other value is dropped.
`text` | A substring match, folded the way the search box folds.
`number` | A whole number, matched exactly.
`date` | A `YYYY-MM-DD` day; a date column and a stored timestamp both answer.

Each type, with its example, is on [Filter Scopes](../elements/filter-scopes.md).

## Declare a filter from code

```rust
use laterite_admin::list::{FilterOption, ListFilter};

let status = ListFilter::select("status", "Status", vec![
    FilterOption::new("draft", "Draft"),
    FilterOption::new("live", "Live"),
]).default_value("live");
let created = ListFilter::date("created_at", "Created").require("acme.blog.audit");
```

Constructor | Type
--- | ---
`ListFilter::boolean(field, label)` | `boolean`
`ListFilter::select(field, label, options)` | `select`
`ListFilter::text(field, label)` | `text`
`ListFilter::number(field, label)` | `number`
`ListFilter::date(field, label)` | `date`
`ListFilter::of(field, label, type)` | Any declared type, with no options.

Method | Sets
--- | ---
`.column(name)` | `column`
`.at_least()`, `.at_most()` | `match`
`.related(name)` | `related`
`.default_value(value)` | `default_value`
`.require(permission)` | `permission`

## Carry a filter in a URL

```text
/admin/posts?f_status=draft&f_featured=1&q=launch&sort=created_at&dir=desc
```

Parameter | Value
--- | ---
`f_<name>` | The filter's value.
`q` | The search.
`sort`, `dir` | The sort column and `asc` or `desc`.

The [export](export.md) link carries the same parameters and downloads the
list the screen shows.
