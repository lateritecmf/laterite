# Page Layout

The heading, side panel, dialogs, split panes and chrome builders a contributed
screen composes its page from.

## Head a page

```rust
use laterite_admin::PageHead;

let actions = Markup::from_template(&ActionsTemplate { shell: &shell })?;
shell.page_with(
    PageHead::new(t!("Places")).back(t!("Regions"), ctx.url("")).actions(actions),
    body,
)
```

```html
<a class="lat-btn lat-btn--secondary lat-btn--sm" href="{{ export_href }}">{{ shell.t("Export") }}</a>
<details class="lat-menu">
  <summary class="lat-btn lat-btn--secondary lat-btn--sm">{{ shell.t("Manage") }} &#9662;</summary>
  <div class="lat-menu__panel">
    <button type="button" data-lat-dialog="new-place">{{ shell.t("New place") }}</button>
    <div class="lat-menu__divider"></div>
    <form method="post" action="{{ purge_href }}"><input type="hidden" name="_csrf" value="{{ shell.csrf_token() }}"><button class="is-danger" type="submit" data-lat-confirm='{{ shell.t("Purge every place?") }}'>{{ shell.t("Purge") }}</button></form>
  </div>
</details>
```

Method | Does
--- | ---
`PageHead::new(title)` | The heading.
`.back(label, href)` | A link above the heading to the page this one sits under.
`.actions(markup)` | Controls in the band under the heading: buttons, a `details.lat-menu`, dialog openers.
`.aside(label, markup)` | A context side panel beside the body, folded under `label` on a narrow window.
`.assets(assets)` | Scripts and styles the page needs, such as `CODE_EDITOR_ASSETS`.
`shell.page_with(head, body)` | The body under that head, inside the chrome.

## Add a side panel

```rust
let panel = Markup::from_template(&RegionsTemplate { shell: &shell, regions })?;
shell.page_with(
    PageHead::new(t!("Places")).aside(t!("Regions"), panel),
    body,
)
```

```html
<nav class="lat-pane" aria-label='{{ shell.t("Regions") }}'>
  <ul class="lat-nav">
    {% for r in regions %}<li><a class="lat-nav__item{% if r.active %} is-active{% endif %}" href="{{ r.href }}">{{ r.name }}</a></li>{% endfor %}
  </ul>
</nav>
```

## Open a form in a dialog

```html
<button type="button" class="lat-btn lat-btn--primary lat-btn--sm" data-lat-dialog="new-place">{{ shell.t("New place") }}</button>
<button type="button" class="lat-btn lat-btn--ghost lat-btn--sm" data-lat-dialog="new-place" data-lat-dialog-set="kind=city">{{ shell.t("New city") }}</button>

<dialog class="lat-dialog" id="new-place">
  <form method="post" action="{{ base }}">
    <div class="lat-dialog__head"><h2>{{ shell.t("New place") }}</h2><button type="button" class="lat-dialog__close" data-lat-dialog-close aria-label='{{ shell.t("Close") }}'>&times;</button></div>
    <div class="lat-dialog__body">
      <input type="hidden" name="_csrf" value="{{ shell.csrf_token() }}">
      <div class="lat-field">
        <label class="lat-field__label" for="kind">{{ shell.t("Kind") }}</label>
        <select class="lat-input lat-select" id="kind" name="kind"><option value="city">{{ shell.t("City") }}</option></select>
      </div>
      <div class="lat-field">
        <label class="lat-field__label" for="name">{{ shell.t("Name") }}</label>
        <input class="lat-input" id="name" name="name" required autofocus>
      </div>
    </div>
    <div class="lat-dialog__actions"><button class="lat-btn lat-btn--ghost" type="button" data-lat-dialog-close>{{ shell.t("Cancel") }}</button><button class="lat-btn lat-btn--primary" type="submit">{{ shell.t("Create") }}</button></div>
  </form>
</dialog>
```

Attribute | On | Does
--- | --- | ---
`data-lat-dialog="id"` | Any control | Opens the `dialog.lat-dialog` with that id and focuses its first field.
`data-lat-dialog-set="field=value;other=value"` | The opener | Fills the named fields before opening.
`data-lat-dialog-close` | A control inside | Closes the dialog. Escape and a click on the backdrop close it too.
`.lat-dialog--wide` | The dialog | A wider dialog.

## Split a screen

```html
<div class="lat-split">
  <nav class="lat-pane">
    <div class="lat-pane__head">{{ shell.t("Regions") }}<button type="button" class="lat-btn lat-btn--ghost lat-btn--sm lat-btn--icon" data-lat-dialog="new-region">{{ shell.icon("plus")|safe }}</button></div>
    <ul class="lat-nav">
      <li><a class="lat-nav__item is-active" href="{{ href }}"><span class="lat-nav__label">{{ shell.t("North") }}</span><span class="lat-nav__meta">12</span></a></li>
    </ul>
  </nav>
  <div class="lat-pane">
    <div class="lat-pane__bar"><nav class="lat-breadcrumb"><a href="{{ base }}">{{ shell.t("All") }}</a></nav><form method="get" action="{{ base }}"><input class="lat-input lat-listbar__search" type="search" name="q"></form></div>
    <div class="lat-pane__body">{{ content|safe }}</div>
    <div class="lat-pane__foot"><button class="lat-btn lat-btn--primary" type="submit">{{ shell.t("Save") }}</button></div>
  </div>
</div>
```

Class | Renders
--- | ---
`lat-split` | A pane beside the content inside a body; `lat-split--aside` puts the pane after the content. One column on a narrow window. For a panel beside the whole page, `PageHead::aside` above.
`lat-pane` | A framed block; `lat-pane__head`, `lat-pane__bar`, `lat-pane__body`, `lat-pane__foot` stack inside it.
`lat-nav`, `lat-nav__item`, `.is-active` | A list of links in a pane, with `lat-nav__label` and `lat-nav__meta`.
`shell.icon(name)` | An icon from the framework's set, inline.

## Compose chrome

```rust
use laterite_admin::ui::{Badge, Button, Card, Toolbar};

let body = Toolbar::new("Reports")
    .button(Button::link("New report", format!("{}/reports/new", ctx.admin_path())).primary())
    .button(Button::post("Rebuild", format!("{}/reports/rebuild", ctx.admin_path())).confirm("Rebuild every report?"))
    .render(&shell);
let card = Card::new(Markup::from_override(table_html)).render();
let state = Badge::new("Stale").render(&shell);
```

Element | Builders | Renders
--- | --- | ---
`Button::link(label, href)` | `.primary()`, `.danger()`, `.ghost()`, `.small()`, `.icon(name)` | A link styled as a button.
`Button::post(label, action)` | The same, and `.confirm(question)` | A form that posts with the request token, asking first when `confirm` is set.
`Toolbar::new(title)`, `Toolbar::section(title)` | `.button(..)` | A page or section heading, its buttons in the band under it.
`Badge::new(label)` | | A small label.
`Card::new(markup)` | | The panel's framed block around what it is given.

## Put a checklist on a screen

```rust
use laterite_admin::checklist::{Checklist, Choice, Expand, Offer};
use laterite_core::Translator;

let list = Checklist::new("Channels")
    .choice(Choice::new("channel", "email", "Email").checked(true))
    .group("Chat", vec![
        Choice::new("channel", "slack", "Slack").note("One workspace").into(),
        Choice::new("channel", "teams", "Teams").locked(true).into(),
    ])
    .select_all(Offer::Auto)
    .search(Offer::Never)
    .expand(Expand::All);

// `Markup`, for a template's `|safe` slot
let html = list.render(&Translator::new("en"));
```

Builder | Does
--- | ---
`Choice::new(name, value, label)` | One box, submitted as `name=value`.
`.note(..)`, `.code(..)` | A second line under the label; an identifier beside it.
`.checked(..)`, `.locked(..)` | Ticked; shown and not changeable.
`.group(label, entries)` | A heading over entries of its own, nested to any depth.
`.select_all(..)`, `.search(..)`, `.expand(..)` | The defaults listed under [Behaviours](../reference/behaviours.md).
`.read_only(true)` | Every box locked.
`.view(&translator)` | The resolved list as a serialisable value, for a field type's view-model.
