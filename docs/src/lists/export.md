# Export

A resource opts into exporting its list as a file; the download carries the
rows the screen shows, minus the paging.

```yaml
# descriptors/posts.yaml
list:
  exportable: true
```

```rust
ListConfig::new("posts", "Posts", columns).exportable()
```

Key | Default | Description
--- | --- | ---
`exportable` | `false` | Mounts `/export` under the resource and shows the export menu beside New.

## Choose the format

```text
GET /admin/posts/export?format=csv&q=launch&sort=created_at&dir=desc&f_status=draft
```

Format | Headed by | Content type
--- | --- | ---
`csv` (default) | The column labels, localized. | `text/csv`
`json` | The column names. | `application/json`

The file carries the columns the user can see and has chosen, the search, the
filters and the sort of the list it was offered from. An unknown `format` is
CSV.

## Keep within the limit

Limit | Value
--- | ---
Rows | 20 000

Past it the export is refused with a message asking to narrow the list.

## Gate it

Who | Gets
--- | ---
A user holding the resource's `permission` | The export menu and the route.
A user without a column's `permission` | A file without that column.
Anyone else | `403 Forbidden`.
