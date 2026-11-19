# Palette Modes

A palette mode is a tab in the command palette (Ctrl+K, Cmd+K) that posts
what a person types to a route of yours and shows the markup it answers.

## Add a mode

```rust
use laterite_admin::palette::PaletteMode;
use laterite_admin::AdminRegistry;
use laterite_core::t;

fn register(&self, registry: &mut laterite_core::Registry) {
    registry.add_palette_mode(
        PaletteMode::new("acme.lookup", t!("Look up"), "/acme/lookup/ask")
            .placeholder(t!("An order number"))
            .prefix('#')
            .permission("acme.orders.view")
            .live(),
    );
}
```

Method | Sets | Default
--- | --- | ---
`new(code, label, path)` | the dotted code with the module's id first, the tab's name, the route under the admin root | required
`placeholder` | the hint in the empty box | none
`prefix` | a character that, typed first, switches to the mode | none
`permission` | the permission a person needs to be offered the mode | every signed-in person
`live()` | posts as the person types | posts on Enter

## Answer the palette

```rust
use axum::{response::Html, routing::post, Form, Router};

#[derive(serde::Deserialize)]
struct Asked {
    q: String,
}

async fn ask(Form(asked): Form<Asked>) -> Html<String> {
    let number: u64 = asked.q.trim().parse().unwrap_or_default();
    Html(format!(
        r#"<ul class="lat-palette__list" role="listbox">
  <li><a class="lat-palette__hit" role="option" href="/admin/acme/orders/{0}">
    <span class="lat-palette__label">Order {0}</span>
    <span class="lat-palette__hint">Open</span>
  </a></li>
</ul>"#,
        number
    ))
}

fn mount(&self) -> Router {
    Router::new().route("/ask", post(ask))
}
```

The route is a [screen](../screens/admin-screens.md) mounted at the mode's path. The
post carries the form field `q`.

Markup | Does
--- | ---
`class="lat-palette__list"` with `lat-palette__hit` links | a result list the arrow keys walk and Enter opens
`class="lat-palette__empty"` | a line for no result
`class="lat-palette__card"` | a form, a card or a result with a title
`data-palette-raise` on the answer's root | moves the palette to the top of the screen and widens it
`data-palette-focus` on one control | takes the focus when the answer arrives
`hx-target="closest .lat-palette__canvas"` | swaps the next answer into the same panel

```html
<form class="lat-palette__card" data-palette-raise
      hx-post="/admin/acme/lookup/refund"
      hx-target="closest .lat-palette__canvas" hx-swap="innerHTML">
  <h2 class="lat-palette__title">Refund order 1042</h2>
  <p class="lat-palette__summary">Returns 40.00 USD to the card.</p>
  <div class="lat-actions">
    <button class="lat-btn lat-btn--danger" type="submit" data-palette-focus>Refund</button>
  </div>
</form>
```

## List the modes

```sh
curl -b cookies.txt https://acme.com/admin/palette/modes
```

```json
[
  { "code": "backend.do", "label": "Do", "placeholder": "What do you want to do?",
    "prefix": ">", "url": "/admin/palette/do", "live": true },
  { "code": "acme.lookup", "label": "Look up", "placeholder": "An order number",
    "prefix": "#", "url": "/admin/acme/lookup/ask", "live": true }
]
```

## The Do mode

`backend.do` (prefix `>`) lists the [abilities](abilities.md) a person may
run and walks one through its input, its preview and its result.

Ability | In Do
--- | ---
Input of strings, numbers, booleans and `enum` choices | offered, one control per property
Input with an object or an array | left out
`Effect::Read` | runs when chosen
Any other effect | shows its `Preview`, then runs on confirmation
`Effect::Delete`, `Effect::Send` | the confirmation is a danger button
`backend.records.*` | left out

A property's `title` is its label and its doc comment the help under it.
A name or a choice with no title reads as words: `found_on_site` shows as
"Found on site".
