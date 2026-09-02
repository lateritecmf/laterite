// Reference picker: typeahead over a PickerSource. Hydrates the current label on
// load, searches as you type, and writes the chosen id to a hidden input.
// Degrades safely: with JS off the hidden input keeps its stored value.
window.lat.widget('ref-picker', function (root) {
  var hidden = root.querySelector('[data-refpicker-id]');
  var search = root.querySelector('[data-refpicker-search]');
  var menu = root.querySelector('[data-refpicker-menu]');
  if (!hidden || !search || !menu) return;
  var searchUrl = root.getAttribute('data-search');
  var resolveUrl = root.getAttribute('data-resolve');
  var currentLabel = '';

  // A QUERY fetch of JSON. The method is uppercase (fetch does not normalise a
  // custom method); a non-JSON reply (e.g. a login redirect on an expired
  // session) yields null rather than a parse error.
  function query(url, payload) {
    return fetch(url, {
      method: 'QUERY',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    }).then(function (resp) {
      var ct = resp.headers.get('content-type') || '';
      if (!resp.ok || ct.indexOf('application/json') === -1) return null;
      return resp.json();
    }).catch(function () { return null; });
  }

  function closeMenu() {
    menu.hidden = true;
    menu.innerHTML = '';
    root.classList.remove('is-open');
  }

  function choose(node) {
    hidden.value = node.id;
    currentLabel = node.label;
    search.value = node.label;
    closeMenu();
  }

  function renderMenu(items) {
    menu.innerHTML = '';
    if (!items || !items.length) {
      closeMenu();
      return;
    }
    items.forEach(function (node) {
      var li = document.createElement('li');
      li.className = 'lat-refpicker__item';
      li.textContent = node.hint ? node.label + '  ·  ' + node.hint : node.label;
      li._node = node;
      menu.appendChild(li);
    });
    menu.hidden = false;
    root.classList.add('is-open');
  }

  // Any mousedown inside the menu keeps the input focused (so the blur-revert
  // never fires mid-selection, even on a click that lands on the menu's padding);
  // the click then selects whichever item was hit.
  menu.addEventListener('mousedown', function (e) {
    e.preventDefault();
  });
  menu.addEventListener('click', function (e) {
    var li = e.target.closest('.lat-refpicker__item');
    if (li && li._node) choose(li._node);
  });

  // Show the stored reference's label.
  if (hidden.value) {
    query(resolveUrl, { id: hidden.value }).then(function (data) {
      if (data && data.item) {
        currentLabel = data.item.label;
        search.value = data.item.label;
      }
    });
  }

  var timer = null;
  var seq = 0;
  // Fetch and show candidates for the query; an empty query shows the first ones.
  function runSearch(q) {
    var mine = ++seq;
    query(searchUrl, { q: q, limit: 20 }).then(function (data) {
      if (mine !== seq) return; // a newer request superseded this one
      renderMenu(data && data.items);
    });
  }

  // Focusing opens the list (a searchable dropdown, not just autocomplete) and
  // selects the shown label so typing replaces it.
  search.addEventListener('focus', function () {
    search.select();
    runSearch('');
  });
  search.addEventListener('input', function () {
    clearTimeout(timer);
    var q = search.value.trim();
    timer = setTimeout(function () {
      runSearch(q);
    }, 200);
  });

  // Leaving the box without choosing reverts it to the committed selection, so a
  // half-typed query never desyncs from the id the form will submit.
  search.addEventListener('blur', function () {
    setTimeout(function () {
      closeMenu();
      search.value = currentLabel;
    }, 150);
  });
});
