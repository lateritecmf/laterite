# Branding

**Settings › Branding**, saved by a user with `backend.manage_branding`. A blank
name keeps [`app.name`](../configuration/application.md).

## Set the fields

Field | Takes | Effect
--- | --- | ---
Application name | text | the brand in the top bar, drawer, sign-in page and titles; blank keeps `app.name`
Logo | a library image | shown in place of the mark
Browser icon | a library image | the tab icon
Accent colour | `#rrggbb` | `--accent`, its pressed shades and tints, and link colours
Colour mode | `auto`, `light`, `dark` | what a user sees before choosing their own
Sign-in prompt | text | the line above the sign-in form
Sign-in tagline | text | the line on the panel beside the form
Sign-in background | a library image | behind that panel
Show the panel | on, off | off leaves the form alone, centred
Custom stylesheet | CSS | added to every admin page; `@import`, scripts and closing tags are removed

Library images come from a public disk. See [Storage Disks](../services/storage-disks.md).
