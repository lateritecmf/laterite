# Colour Mode

A site offers light, dark, or follow the system, resolved before the first paint and kept in step with the system while the page is open.

```jinja
<head>
  {{ colour_mode() }}
  <link rel="stylesheet" href="{{ asset('css/site.css') }}">
</head>
```

```css
html[data-theme="dark"] { color-scheme: dark; background: #111; color: #eee; }
html[data-theme="light"] { color-scheme: light; background: #fff; color: #111; }
```

```html
<button onclick="latSetMode('dark')">Dark</button>
<button onclick="latSetMode('light')">Light</button>
<button onclick="latSetMode('auto')">System</button>
```

Function | Emits
--- | ---
`colour_mode()` | The inline script, to place before any stylesheet.
`colour_mode(file=true)` | A `<script src="/laterite/theme-boot.js">` tag, for a Content Security Policy that forbids inline scripts; the site serves the file and a static build writes it.

Name | Holds
--- | ---
`<html data-theme="light">` | The resolved mode; write CSS against it.
`localStorage["lat-mode"]` | The reader's choice: `light`, `dark` or `auto`.
`latMode()` | The current choice.
`latSetMode(mode)` | Persists a choice and applies it.
`latApplyMode(mode)` | Applies a mode without persisting it.

## Use it from Rust

```rust
use laterite_core::theme;

let head = format!("<head>{}<link rel=\"stylesheet\" href=\"/site.css\"></head>", theme::boot_markup());
```

Name | Gives
--- | ---
`theme::boot_markup()` | The inline `<script>` element.
`theme::BOOT_JS` | The script alone, to serve as a file or embed with a nonce.
`theme::MODE_KEY` | `lat-mode`.
`theme::MODES` | `["light", "dark", "auto"]`.

A deferred or external script runs after the first paint, and a dark-mode reader sees the page flash white; the inline form avoids that.
