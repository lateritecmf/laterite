# Dates and Timezones

Laterite stores every timestamp in UTC and converts it to a display timezone
only when rendering. Nothing about how a date is shown ever changes what is
stored, so timezones are purely a presentation concern.

## How a timestamp is displayed

List columns declare a kind. A column marked as a datetime is parsed from its
stored UTC value, converted to the viewer's timezone, and formatted
human-readably (for example `14 Aug 2026, 15:53`) instead of the raw ISO string:

```rust
use laterite_admin::list::ListColumn;

ListColumn::new("created_at", "Created").datetime();
ListColumn::new("published_on", "Published").date();
ListColumn::new("is_active", "Active").yes_no();
```

The kinds are `text` (the default), `datetime`, `date`, `time`, and a `yes_no`
boolean. A value that cannot be parsed falls back to the raw string rather than
erroring.

## Which timezone is used

The display timezone is resolved for each request in two tiers:

1. The signed-in operator's own preference, if they have set one.
2. Otherwise the deployment default from
   [`backend.timezone`](configuration.md) (an IANA name such as
   `Asia/Kolkata`), which itself falls back to `UTC`.

An operator sets their own timezone from **Preferences** (the user menu, top
right). Choosing a zone makes every date in the admin render in it for that
operator only; choosing "Use the deployment default" clears the preference so
they follow `backend.timezone` again. Because storage is always UTC, switching
timezones never migrates or rewrites any data.
