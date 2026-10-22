# JavaScript API

The `window.lat` helpers a contributed screen or widget calls, the events every
island announces, and the attributes that join a form of your own in.

## Raise a message from script

```js
window.lat.flash('Import finished.', 'success');   // or 'error', which stays until dismissed
```

## Confirm before acting

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

## Join a form in

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
`data-lat-trigger-action`, `data-lat-trigger-field`, `data-lat-trigger-condition` | On a field's wrapper: the dependency, as [Field Dependencies](../forms/field-dependencies.md) spells it.

## Keep Enter inside a widget

```html
<div data-lat-widget="acme-lookup" data-lat-enter-scope data-lat-enter-target=".acme-lookup__go">
  <input type="search" name="q">
  <button type="button" class="acme-lookup__go">Look up</button>
</div>
```

Attribute | Does
--- | ---
`data-lat-enter-scope` | On the root of a widget holding an input: Enter acts there and never submits the form around it.
`data-lat-enter-target="<selector>"` | The button Enter presses inside that scope. Without a target, Enter does nothing.

## Listen and drive

Every island announces what it does as a DOM event named
`lat:<component>:<event>`, bubbling from its root. Server-side facts are on the
[event bus](../services/events.md).

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

// A rich editor: the same three, plus the engine for anything else.
var rich = lat.get(document.querySelector('[data-lat-widget="richeditor"]'));
rich.editor.commands.insertContent('<p>Hello</p>');

// A markdown editor: the same, plus the Preview tab.
lat.get(document.querySelector('[data-lat-widget="markdown"]')).preview();

// A code editor: read, replace, focus.
var code = lat.get(document.querySelector('[data-lat-widget="codeeditor"]'));
code.set(code.get().trim());
code.focus();
```

Event | Detail | Cancelable
--- | --- | ---
`repeater:before-add` | `count` | Yes
`repeater:added` | `row`, `index`, `count` | No
`repeater:before-remove` | `row`, `index`, `count` | Yes
`repeater:removed` | `index`, `count` | No
`repeater:moved` | `row`, `from`, `to`, `count` | No
`repeater:duplicated` | `row`, `index`, `count` | No
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
Repeater | `add()`, `remove(index)`, `move(from, to)`, `duplicate(index)`, `count()`
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
`lat.flash(text, level)` | Raises a message: `success`, or `error`, which stays until dismissed.
`lat.assets.ensure(url)` | Loads a stylesheet or script once.

## Put a code editor on a contributed screen

```html
<div data-lat-widget="codeeditor" data-language="yaml" style="--rows:16">
  <textarea class="lat-input lat-editor" id="spec" name="spec" rows="16" data-lat-grow="off">{{ spec }}</textarea>
</div>
```

```rust
shell.page_with(PageHead::new("Specification").assets(laterite_admin::CODE_EDITOR_ASSETS), body)
```

Attribute | Values | Default
--- | --- | ---
`data-language` | `text`, `html`, `css`, `javascript`, `json`, `yaml`, `markdown`, `jinja` | `text`
`data-line-numbers` | `off` | on
`data-wrap` | `on` | off
`--rows` | lines | `12`
`readonly` on the textarea | | A read-only editor

Without the script the textarea is the control.

## Choose from the library

```js
var picker = lat.mediaPicker({
  multiple: false,
  words: { title: 'Choose from the library', use: 'Use selected', cancel: 'Cancel' },
  onDone: function (items) { console.log(items[0].id, items[0].url); }
});
picker.open('/admin/media/pick?mode=image');
```

Option | Values
--- | ---
`multiple` | `true`, `false`
`words` | `title`, `use`, `cancel`
`onDone(items)` | each item: `id`, `name`, `kind`, `thumb`, `url`, `is_image`
`open(url)` | the picker fragment, with `?mode=image` for images alone

## Write an island

```js
window.lat.widget('char-count', function (el) {
  var input = el.querySelector('input');
  function count() { return input.value.length; }
  input.addEventListener('input', function () {
    el.querySelector('.count').textContent = count();
    lat.emit(el, 'acme.char-count:changed', { count: count() });
  });
  // Returned: `lat.get(el).count()` reads it from outside.
  return { count: count };
});
```

```html
<div data-lat-widget="char-count">
  <input type="text" name="title">
  <span class="count">0</span>
</div>
```

Return | Does
--- | ---
The controller object | What `lat.get(el)` hands out.
`root` | The markup an island built around its element; `lat.get` then finds the controller from inside it too.

Bind by structure, not by id: the same markup may appear more than once on a
page and inside swapped content.
