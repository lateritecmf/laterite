# Branding

**Settings › Branding**, saved by a user with `backend.manage_branding`. A blank
name keeps [`app.name`](../configuration/application.md).

## Set the fields

Field | Takes | Effect
--- | --- | ---
Application name | text | the brand in the top bar, drawer, sign-in page and titles; blank keeps `app.name`
Logo | a library image | shown in place of the mark
Navigation logo | a library image | shown in the top bar and drawer; blank uses the logo
Browser icon | a library image | the tab icon
Palette | `clay`, `slate`, `forest`, `ocean`, `plum`, `amber` | a ready-made accent; an accent colour wins over it
Accent colour | `#rrggbb` | `--accent`, its pressed shades and tints, and link colours
Colour mode | `auto`, `light`, `dark` | what a user sees before choosing their own
Offer the colour mode button | on, off | off removes the button from the top bar; everyone gets the colour mode above
Sign-in prompt | text | the line above the sign-in form
Sign-in tagline | text | the line on the panel beside the form
Sign-in background | a library image | behind that panel
Sign-in background colour | `#rrggbb` | a flat colour for that panel, when no image is set
Show the panel | on, off | off leaves the form alone, centred
Custom stylesheet | CSS | added to every admin page; `@import`, scripts and closing tags are removed

Each image is a [`public` media field](../media/field.md): the picker takes public files alone. See [Storage Disks](../services/storage-disks.md).

## Keep a baseline with the code

```toml
# config/default.toml
[backend.brand]
palette = "ocean"
accent = "#1F6FB2"
mode = "dark"
show_mode_switch = false
login_prompt = "Sign in to Acme"
login_tagline = "Orders, stock and customers."
login_background_color = "#102030"
```

Value | Comes from
--- | ---
Set on the screen | The screen.
Blank on the screen | `[backend.brand]`.
In neither | The framework's default.

The images and the custom stylesheet are set on the screen alone. **Reset to
defaults** on the screen clears what it saved; the file's values show again.
