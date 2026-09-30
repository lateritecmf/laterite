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
Enter | A picker's search box, a repeater row | Acts there: the picker chooses what is highlighted; a repeater row moves to the next field, and from its last field adds a row. Never submits the form around it.
Down, Up, Home, End | A picker's list | Move the highlight. Down on a closed list opens it.
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

## Ask from script

```js
lat.confirm('Archive 12 articles?', { go: 'Archive', cancel: 'Keep', focus: 'cancel' }, function (yes) {
  if (yes) archive();
});
```

Label | Does
--- | ---
`go` | Names the confirming button. Default `OK`.
`cancel` | Names the other. Default `Cancel`, localized.
`focus` | `'cancel'` opens with the cursor on it. Default: the confirming button.

## Forms

A form of your own template joins in with one attribute:

```html
<form method="post" action="/admin/acme/import" data-lat-widget="form" data-lat-focus="first">
```

Attribute | Does
--- | ---
`data-lat-widget="form"` | Asks before leaving with changes unsaved.
`data-lat-refused` | The form holds a submission that was not saved: it opens changed, with the cursor in the first field carrying an error.
`data-lat-focus` | `first` opens with the cursor in the first field; `off` leaves it alone.
`data-lat-confirm-leave="off"` | Never asks.
`data-lat-counter` | On a control with `maxlength`: `auto` or `on`.
`data-lat-grow="off"` | On a textarea: keeps the height it was given.
`data-lat-preset`, `data-lat-preset-type` | On a field's wrapper: the field followed and the shape (`slug`, `url`, `file`, `exact`).
`data-lat-trigger-action`, `data-lat-trigger-field`, `data-lat-trigger-condition` | On a field's wrapper: the dependency, as [Descriptor Files](descriptor-files.md#dependencies) spells it.

## Events

Every island announces what it does as a DOM event named
`lat:<component>:<event>`, bubbling from its root. Server-side facts are on the
[event bus](events.md).

```js
var stop = lat.on('repeater:added', function (detail, event) {
  console.log(detail.count, 'rows; the new one is', detail.row);
});

// A `before-` event is cancelable: this caps a repeater at five rows.
lat.on('repeater:before-add', function (detail, event) {
  if (detail.count >= 5) event.preventDefault();
});

// Drive an island through its controller.
lat.get(document.querySelector('[data-lat-widget="repeater"]')).add();
```

Event | Detail | Cancelable
--- | --- | ---
`repeater:before-add` | `count` | Yes
`repeater:added` | `row`, `index`, `count` | No
`repeater:before-remove` | `row`, `index`, `count` | Yes
`repeater:removed` | `index`, `count` | No
`ref-picker:changed` | `id`, `label`, `previous` | No
`ref-picker:cleared` | `previous` | No
`selection:changed` | `ids`, `count` | No
`checklist:changed` | `values`, `count`, `total` | No
`flash:shown` | `text`, `level` | No
`flash:dismissed` | `text` | No
`confirm:opened` | `text` | No
`confirm:confirmed`, `confirm:cancelled` | None | No
`copy:copied` | `value` | No
`reveal:shown`, `reveal:hidden` | None | No
`preset:filled` | `from`, `value` | No
`trigger:changed` | `field`, `met` | No

Controller | Methods
--- | ---
Repeater | `add()`, `remove(index)`, `count()`
Record picker, searched select | `value()`, `label()`, `choose({ id, label })`, `clear()`
Checklist | `values()`, `count()`, `set(values)`, `all()`, `none()`, `search(text)`
Form | `changed()`, `settle()`: takes the form as it stands for saved
Password reveal | `show()`, `hide()`

Helper | Does
--- | ---
`lat.on(name, handler)` | Listens page-wide, swapped content included. Returns the function that stops listening.
`lat.emit(el, name, detail, cancelable)` | Announces `lat:<name>` from `el`. Returns `false` when a listener cancelled it.
`lat.get(el)` | The controller of the island at or around `el`.
`lat.scan(root)` | Starts the islands in markup a script added. Swapped content starts without it.
`lat.confirm(text, labels, done)` | Asks in the admin's dialog. `done` hears `true` or `false`.

## Write an island

```js
window.lat.widget('char-count', function (el) {
  var input = el.querySelector('input');
  function count() { return input.value.length; }
  input.addEventListener('input', function () {
    el.querySelector('.count').textContent = count();
    lat.emit(el, 'acme.char-count:changed', { count: count() });
  });
  // Returned, so `lat.get(el).count()` reads it from outside.
  return { count: count };
});
```

An island that builds markup around its element returns that markup as `root`,
so `lat.get` finds the controller from inside it too.

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
