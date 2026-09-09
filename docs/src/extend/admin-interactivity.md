# Admin Interactivity

The admin is server-rendered HTML with [htmx](https://htmx.org) for in-place
updates. There is no build step and no client framework. Behaviour that htmx
cannot express is written as a small island, registered by name.

## Request feedback

Every htmx request in the admin gets three things without any markup:

- A thin progress bar across the top of the viewport while the request is in
  flight. It creeps rightward, since the duration is unknown, and fades when the
  last in-flight request finishes.
- The submitting form takes htmx's `htmx-request` class, which dims its buttons.
  Descriptor forms also carry `hx-disabled-elt`, so the submit button is disabled
  for the round trip and a double click posts once.
- A failure raises a toast. htmx discards a response it will not swap, so a `500`
  or a dropped connection would otherwise leave a click with no outcome.

A `422` is exempt from the failure toast: it carries the form back with its own
per-field errors, which is the [validation](validation.md) contract, not a
failure.

## Raising a toast from script

`window.lat.flash(text, level)` adds a toast matching the server-rendered ones.
`level` is `success` or `error`; an error toast stays until dismissed.

```js
window.lat.flash('Import finished.', 'success');
```

A flash set on the server survives a redirect and needs no script; reach for this
only when something completes without a page change.

## Lists

A list screen wraps its table and pager in one region, `#lat-list`. Column
headers and pager links `hx-get` the same URL the link points at and swap that
region, with `hx-push-url` so the address bar tracks the sort and page. The back
button and a refresh both land on the same view, and a copied URL opens it.

Every header sorts. A click orders by that column ascending, and a second click
on the sorted column flips it. Only a column the descriptor declares can be
sorted: a `sort` naming anything else falls back to the descriptor's own order,
so the query never orders by an arbitrary column.

The search box asks as you type, on a short debounce, and sits outside the
swapped region so a result never steals the caret. It looks in the columns the
descriptor marks searchable, which by default means the text ones: a substring
match against a boolean or a stored timestamp answers nonsense. Override it per
column with `ListColumn::searchable`. A sort or a page keeps the term, and a
blank term is no filter rather than a filter matching nothing.

Filters sit in the same bar. A filter declares a column and what it offers
(`ListFilter::boolean`, or `ListFilter::select` with a fixed set of options), and
only a declared filter, and for a select only a declared option, reaches the
query. Changing one re-asks immediately; the term, the filters and the sort all
travel together on every link.

The bar sits outside the swapped region, so it cannot re-render per swap. The
region instead declares `data-filtered` and the stylesheet reads it, which is how
the Clear link appears only when something is narrowing the list. Prefer that
shape over an out-of-band swap for anything the region already knows.

With scripting off the headers, pager and bar stay ordinary links and a GET form,
and the same handler answers the whole page.

Each operator chooses which columns a list shows, from a native disclosure in the
bar, and the choice is remembered against their account. It narrows the descriptor
once per request, so what is shown is also what is queried, sorted and searched.
Picking every column clears the preference rather than storing them all, so the
operator keeps following the descriptor as it gains columns; a stored choice that
no longer matches any declared column falls back to showing everything.

## Confirming a destructive action

A control carrying `data-lat-confirm` asks before it acts:

```html
<button type="submit" data-lat-confirm="Delete the selected records? This cannot be undone.">
  Delete
</button>
```

The click is caught in the capture phase, so neither the form nor htmx sees it
until the operator confirms; Cancel and Escape close the dialog and nothing is
sent. It is a modal rather than `window.confirm` because a native dialog blocks
the page and cannot carry the admin's styling or its translations.

## Islands

An island is a named initialiser. Every element carrying a matching
`data-lat-widget` is initialised once, on first load and again after an htmx
swap, so a fragment brings its widgets up with it:

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

Initialise idempotently and bind by structure rather than by id, so an element
that appears twice on a page still works.

`window.lat.assets.ensure(url)` loads a stylesheet or script once, for a fragment
whose assets are not already on the page.

## What is not built yet

Inline editing in a list row, and reordering columns (the picker chooses which,
not where).
