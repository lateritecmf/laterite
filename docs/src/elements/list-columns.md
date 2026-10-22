# List Columns

Every built-in column type. None reads a key of its own; the keys every column
takes (`label`, `width`, `labels`, ...) are on [List
Descriptor](../lists/list-descriptor.md), and a type of your own is [Writing a
Column Type](../lists/column-types.md).

## text

```yaml
columns:
  title: { searchable: true }
```

The value as stored, or its entry under `labels`. The type when none is
written; the search box looks in it.

## datetime

```yaml
columns:
  created_at: { type: datetime, align: right }
```

A date and time in the display timezone.

## date

```yaml
columns:
  published_on: { type: date }
```

A date in the display timezone.

## time

```yaml
columns:
  opens_at: { type: time }
```

A time of day in the display timezone.

## boolean

```yaml
columns:
  is_live: { type: boolean, width: 80px }
```

Value | Shown as
--- | ---
`1`, `true` | Yes
`0`, `false`, empty | No

## status_pill

```yaml
columns:
  status: { type: status_pill, labels: [{ value: draft, label: Draft }, { value: live, label: Live }] }
```

A pill coloured by the value, lowercased with anything but letters and digits
made a hyphen, and reading as its entry under `labels`.
