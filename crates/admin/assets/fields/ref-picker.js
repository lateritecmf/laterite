// Typeahead: a search box over a list of choices, with the list under it.
//
// Two islands share it. The record picker asks a picker source over HTTP and
// keeps the chosen id in a hidden input. A searched dropdown keeps its own
// `<select>`, hidden, as the control the form submits, and offers its options
// through the same box. Both degrade: with no script the hidden input keeps
// its stored value, and the `<select>` is a `<select>`.
//
// Keyboard: Down and Up move through the list, Enter chooses (the first match
// when nothing is highlighted), Escape closes and puts the chosen label back,
// Tab leaves. Leaving the box empty clears the choice.
(function () {
  var ids = 0;

  // `source.search(query, done)` hands `done` an array of `{ id, label, hint }`;
  // `source.resolve(id, done)` hands it the one item, or null. `source.choose`
  // and `source.clear` write the choice wherever the form reads it.
  function typeahead(root, source) {
    var search = root.querySelector('[data-refpicker-search]');
    var menu = root.querySelector('[data-refpicker-menu]');
    if (!search || !menu) return null;
    var currentId = source.current();
    var currentLabel = '';
    var active = -1;
    var items = [];

    menu.id = menu.id || ('lat-menu-' + (++ids));
    search.setAttribute('role', 'combobox');
    search.setAttribute('aria-autocomplete', 'list');
    search.setAttribute('aria-expanded', 'false');
    search.setAttribute('aria-controls', menu.id);
    menu.setAttribute('role', 'listbox');

    function closeMenu() {
      menu.hidden = true;
      menu.innerHTML = '';
      items = [];
      active = -1;
      root.classList.remove('is-open');
      search.setAttribute('aria-expanded', 'false');
      search.removeAttribute('aria-activedescendant');
    }

    function highlight(index) {
      var rows = menu.children;
      if (!rows.length) return;
      if (index < 0) index = 0;
      if (index >= rows.length) index = rows.length - 1;
      if (active >= 0 && rows[active]) {
        rows[active].classList.remove('is-active');
        rows[active].setAttribute('aria-selected', 'false');
      }
      active = index;
      rows[active].classList.add('is-active');
      rows[active].setAttribute('aria-selected', 'true');
      search.setAttribute('aria-activedescendant', rows[active].id);
      if (rows[active].scrollIntoView) rows[active].scrollIntoView({ block: 'nearest' });
    }

    function choose(item) {
      var previous = currentId;
      currentId = String(item.id);
      currentLabel = item.label;
      search.value = item.label;
      closeMenu();
      source.choose(item, previous);
    }

    function clear() {
      var previous = currentId;
      if (!previous) return;
      currentId = '';
      currentLabel = '';
      search.value = '';
      closeMenu();
      source.clear(previous);
    }

    function renderMenu(found) {
      menu.innerHTML = '';
      items = found || [];
      active = -1;
      if (!items.length) {
        closeMenu();
        return;
      }
      items.forEach(function (item, index) {
        var li = document.createElement('li');
        li.className = 'lat-refpicker__item';
        li.id = menu.id + '-' + index;
        li.setAttribute('role', 'option');
        li.setAttribute('aria-selected', 'false');
        li.textContent = item.hint ? item.label + '  ·  ' + item.hint : item.label;
        li._index = index;
        menu.appendChild(li);
      });
      menu.hidden = false;
      root.classList.add('is-open');
      search.setAttribute('aria-expanded', 'true');
      // The choice already made leads the list, so Enter keeps it.
      var here = -1;
      items.forEach(function (item, index) { if (String(item.id) === currentId) here = index; });
      if (here >= 0) highlight(here);
    }

    // Any mousedown inside the menu keeps the input focused (so the blur never
    // fires mid-selection, even on a click that lands on the menu's padding);
    // the click then selects whichever item was hit.
    menu.addEventListener('mousedown', function (e) { e.preventDefault(); });
    menu.addEventListener('mousemove', function (e) {
      var li = e.target.closest('.lat-refpicker__item');
      if (li && li._index !== active) highlight(li._index);
    });
    menu.addEventListener('click', function (e) {
      var li = e.target.closest('.lat-refpicker__item');
      if (li && items[li._index]) choose(items[li._index]);
    });

    var seq = 0;
    function runSearch(q) {
      var mine = ++seq;
      source.search(q, function (found) {
        if (mine !== seq) return; // a newer request superseded this one
        renderMenu(found);
      });
    }

    if (currentId) {
      source.resolve(currentId, function (item) {
        if (!item) return;
        currentLabel = item.label;
        search.value = item.label;
      });
    }

    var timer = null;
    // Focusing opens the list (a searchable dropdown, not just autocomplete) and
    // selects the shown label so typing replaces it.
    search.addEventListener('focus', function () {
      search.select();
      runSearch('');
    });
    search.addEventListener('input', function () {
      clearTimeout(timer);
      var q = search.value.trim();
      timer = setTimeout(function () { runSearch(q); }, source.delay || 0);
    });
    search.addEventListener('keydown', function (e) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (menu.hidden) runSearch(search.value.trim()); else highlight(active + 1);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (!menu.hidden) highlight(active - 1);
      } else if (e.key === 'Home' && !menu.hidden) {
        e.preventDefault();
        highlight(0);
      } else if (e.key === 'End' && !menu.hidden) {
        e.preventDefault();
        highlight(items.length - 1);
      } else if (e.key === 'Enter') {
        if (menu.hidden) return;
        e.preventDefault();
        var pick = items[active >= 0 ? active : 0];
        if (pick) choose(pick);
      } else if (e.key === 'Escape') {
        // With the list closed, Escape still puts the chosen label back, so
        // a half-typed query never lingers; the rest of the page hears it too.
        if (!menu.hidden) e.stopPropagation();
        closeMenu();
        search.value = currentLabel;
      }
    });

    // Leaving the box without choosing puts the chosen label back, so a
    // half-typed query never desyncs from the id the form will submit. Leaving
    // it empty is a choice too: the field is cleared.
    search.addEventListener('blur', function () {
      setTimeout(function () {
        closeMenu();
        if (search.value.trim() === '') clear(); else search.value = currentLabel;
      }, 150);
    });

    return {
      value: function () { return currentId; },
      label: function () { return currentLabel; },
      choose: choose,
      clear: clear
    };
  }

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

  // Record picker: a typeahead over a picker source, writing the chosen id to
  // a hidden input.
  window.lat.widget('ref-picker', function (root) {
    var hidden = root.querySelector('[data-refpicker-id]');
    if (!hidden) return;
    var searchUrl = root.getAttribute('data-search');
    var resolveUrl = root.getAttribute('data-resolve');
    return typeahead(root, {
      delay: 200,
      current: function () { return hidden.value; },
      search: function (q, done) {
        query(searchUrl, { q: q, limit: 20 }).then(function (data) { done(data && data.items); });
      },
      resolve: function (id, done) {
        query(resolveUrl, { id: id }).then(function (data) { done(data && data.item); });
      },
      choose: function (item, previous) {
        hidden.value = item.id;
        window.lat.emit(root, 'ref-picker:changed', { id: String(item.id), label: item.label, previous: previous });
      },
      clear: function (previous) {
        hidden.value = '';
        window.lat.emit(root, 'ref-picker:cleared', { previous: previous });
      }
    });
  });

  // Searched dropdown: the `<select>` stays the control the form submits, out
  // of sight, and a typeahead over its options stands in for it. An option
  // with an empty value is what clearing chooses.
  window.lat.widget('select-search', function (select) {
    var root = document.createElement('div');
    root.className = 'lat-refpicker';
    root.setAttribute('data-lat-enter-scope', '');
    var search = document.createElement('input');
    search.className = 'lat-input';
    search.type = 'text';
    search.autocomplete = 'off';
    search.setAttribute('data-refpicker-search', '');
    if (select.id) {
      search.id = select.id;
      select.removeAttribute('id');
    }
    if (select.disabled) search.disabled = true;
    // A hidden control cannot show a validation message, so the box carries
    // the requirement: it is empty exactly when nothing is chosen.
    if (select.required) {
      search.required = true;
      select.required = false;
    }
    var caret = document.createElement('span');
    caret.className = 'lat-refpicker__caret';
    caret.setAttribute('aria-hidden', 'true');
    caret.innerHTML = '&#9660;';
    var menu = document.createElement('ul');
    menu.className = 'lat-refpicker__menu';
    menu.setAttribute('data-refpicker-menu', '');
    menu.hidden = true;
    select.parentNode.insertBefore(root, select);
    root.appendChild(search);
    root.appendChild(caret);
    root.appendChild(menu);
    root.appendChild(select);
    select.hidden = true;
    select.setAttribute('tabindex', '-1');

    function options() {
      return Array.prototype.map.call(select.options, function (o) { return { id: o.value, label: o.text }; });
    }
    var blank = Array.prototype.some.call(select.options, function (o) { return o.value === ''; });

    var controller = typeahead(root, {
      current: function () { return select.value; },
      search: function (q, done) {
        var needle = q.toLowerCase();
        done(options().filter(function (o) {
          return o.id !== '' && (needle === '' || o.label.toLowerCase().indexOf(needle) !== -1);
        }));
      },
      resolve: function (id, done) {
        var found = options().filter(function (o) { return o.id === id; })[0];
        done(found && found.id !== '' ? found : null);
      },
      choose: function (item) {
        if (select.value === item.id) return;
        select.value = item.id;
        select.dispatchEvent(new Event('change', { bubbles: true }));
      },
      clear: function () {
        if (!blank) return;
        select.value = '';
        select.dispatchEvent(new Event('change', { bubbles: true }));
      }
    });
    if (controller) controller.root = root;
    return controller;
  });
})();
