# Admin Interactivity

The admin is server-rendered HTML with [htmx](https://htmx.org). Behaviour
htmx cannot express is an island, registered by name.

## Request feedback

Every htmx request shows a progress bar, dims the submitting form, and raises
a toast on failure. A `422` re-renders the form with its errors. See
[Validation](validation.md).

Raise a toast from script:

```js
window.lat.flash('Import finished.', 'success');   // or 'error', which stays until dismissed
```

## Keyboard

Key | Where | Does
--- | --- | ---
Enter | A single-line field | Submits the form.
Enter | A textarea | A newline.
Cmd/Ctrl+Enter | Anywhere in a form | Submits it.
Enter | A picker's search box, a repeater row | Acts there: the picker keeps searching; a repeater row moves to the next field, and from its last field adds a row. Never submits the form around it.
Escape | A menu, a dialog | Closes it.

Opt out in a descriptor:

```yaml
form:
  enter: off               # Enter never submits this form; Cmd/Ctrl+Enter still does
  fields:
    sku: { enter: next }   # Enter moves to the next field
    notes: { type: textarea }
```

A widget of your own that puts an input inside a form marks its root
`data-lat-enter-scope`, with `data-lat-enter-target="<selector>"` naming the
button Enter presses there; without a target, Enter does nothing.

## Lists

Control | Behaviour
--- | ---
Column header | Sorts; a second click flips it. Only a declared, `sortable` column sorts.
Column | `ListColumn` builders: `sortable(false)`, `invisible()`, `width("10%")`, `align(Align::Right)`, `require(permission)`. A column the operator may not see is not queried, sorted, searched or exported.
Search | Asks as you type, in the columns marked searchable; text columns by default. `ListColumn::searchable` overrides. `ListConfig::search(SearchConfig)` sets a `prompt`, asks `on_enter` only, or turns it `off()`.
Empty state | "No records yet.", or `ListConfig::no_records_message(text)`.
Filters | `boolean`, `select`, `text`, `number`, `date`, or `of(field, label, type)`. Only a value the type accepts reaches the query. `default_value` narrows the list until the operator decides; `require(permission)` gates it.
List setup | Each operator chooses which columns show, and the page size. Picking every column clears the choice.
Export | On a list that calls `.exportable()`: CSV or JSON of the current query without paging. Over 20,000 rows is refused.
Pager | Keeps the sort, search and filters.
Page size | `per_page`. `per_page_options` offers a choice in the list setup, remembered per operator.

Every control is a link or a GET form, so it works with scripting off.

Add a toolbar button:

```rust
use laterite_admin::list::ToolbarButton;

ToolbarButton::new("Reports", "/reports")
    .icon("history")
    .require("acme.view_reports");
```

A path starting with `/` resolves under the admin mount. A button the operator
lacks the permission for is not rendered.

## Confirm a destructive action

```html
<button type="submit" data-lat-confirm="Delete the selected records? This cannot be undone.">
  Delete
</button>
```

## Write an island

```js
window.lat.widget('char-count', function (el) {
  var input = el.querySelector('input');
  input.addEventListener('input', function () {
    el.querySelector('.count').textContent = input.value.length;
  });
});
```

```html
<div data-lat-widget="char-count">
  <input type="text" name="title">
  <span class="count">0</span>
</div>
```

Every matching element is initialised once, on load and after each htmx swap.
Bind by structure, not by id. `window.lat.assets.ensure(url)` loads a
stylesheet or script once.

## Not built yet

Inline editing in a row; reordering columns.
