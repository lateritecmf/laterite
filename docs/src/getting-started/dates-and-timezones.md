# Dates and Timezones

Timestamps are stored in UTC and converted to a display timezone when rendered.

## Display a timestamp

```rust
use laterite_admin::list::ListColumn;

ListColumn::new("created_at", "Created").datetime();
ListColumn::new("published_on", "Published").date();
ListColumn::new("is_active", "Active").yes_no();
```

Kind | Rendered as
--- | ---
`text` | The stored value. Default.
`datetime` | `14 Aug 2026, 15:53`, in the viewer's timezone.
`date` | The date, in the viewer's timezone.
`time` | The time, in the viewer's timezone.
`yes_no` | Yes or No.

A value that does not parse is shown as stored.

## Choose the timezone

Resolved per request:

1. The operator's own preference, set under **Preferences**.
2. Otherwise [`backend.timezone`](configuration.md), an IANA name such as
   `Asia/Kolkata`. Defaults to `UTC`.

"Use the deployment default" in Preferences clears the operator's choice.
