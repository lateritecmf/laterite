# Filter Scopes

Every built-in filter type, with what it offers and what reaches the query.
The keys every filter takes (`label`, `default_value`, `permission`) are on
[Filters](../lists/filters.md).

## boolean

```yaml
filters:
  featured: { type: boolean }
```

Offers | Matches
--- | ---
Yes or no | Rows where the column is true (`1`) or false (`0`).

## select

```yaml
filters:
  status:
    type: select
    options: [{ value: draft, label: Draft }, { value: live, label: Live }]
    default_value: live
```

Key | Description
--- | ---
`options` | The choices, a list of `{ value, label }`. Required.

Offers | Matches
--- | ---
One of the declared values | Rows equal to it. A value not declared is dropped.

## text

```yaml
filters:
  author: { type: text }
```

Offers | Matches
--- | ---
A text box | Rows containing the text, folded the way the search box folds.

## number

```yaml
filters:
  views: { type: number }
```

Offers | Matches
--- | ---
A number box | Rows equal to the whole number. Anything else is dropped.

## date

```yaml
filters:
  created_at: { type: date }
```

Offers | Matches
--- | ---
A date picker | Rows on that `YYYY-MM-DD` day, whether the column holds a date or a timestamp.
