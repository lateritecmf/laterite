// Laterite admin: shared behaviour and the widget (island) lifecycle.

function latModeGlyph(m) {
  return m === 'light' ? '☀' : m === 'dark' ? '☾' : '◐';
}
// The admin's own control over the shared mechanism: `laterite_core::theme`
// owns storing and applying a mode, this only decides what the button does next
// and which glyph it shows. A site builds its own control the same way.
function latCycleMode() {
  var o = latMode();
  var n = o === 'light' ? 'dark' : o === 'dark' ? 'auto' : 'light';
  latSetMode(n);
  var e = document.getElementById('lat-mode-ico');
  if (e) e.textContent = latModeGlyph(n);
}

function latToggleMenu() {
  var m = document.getElementById('lat-menu');
  if (m) m.classList.toggle('is-open');
}
function flashText(toast) {
  var label = toast.querySelector('.lat-flash__text');
  return label ? label.textContent : '';
}
function latDismissFlash(btn) {
  var t = btn.closest('.lat-flash');
  if (!t) return;
  window.lat.emit(t, 'flash:dismissed', { text: flashText(t) });
  t.classList.add('is-leaving');
  setTimeout(function () { t.remove(); }, 180);
}

// htmx ignores a non-2xx response by default, so a form that failed validation
// would post and appear to do nothing. A 422 is our "here is the form again,
// with errors": let it swap, into the element that asked.
document.addEventListener('htmx:beforeSwap', function (e) {
  if (e.detail.xhr.status === 422) {
    e.detail.shouldSwap = true;
    e.detail.isError = false;
  }
});

// Widget (island) registry: register an initialiser by name; every element with
// a matching data-lat-widget is initialised exactly once, on first load and
// after an htmx swap (swapped fragments carry their own widgets).
//
// Islands speak through DOM events named `lat:<component>:<event>`, bubbling
// from the island's root with a detail object, and an island may hand back a
// controller (its methods) from its initialiser. Both are public: `lat.on`
// listens, `lat.emit` announces, `lat.get` finds a controller.
(function () {
  var registry = {};
  var controllers = new WeakMap();
  var lat = (window.lat = window.lat || {});
  lat.widget = function (name, init) {
    registry[name] = init;
    scan(document);
  };
  // The controller the island at `el` (or around it) handed back when it
  // started, or undefined for one with none.
  lat.get = function (el) {
    var root = el && el.closest ? el.closest('[data-lat-widget]') : null;
    return root ? controllers.get(root) : undefined;
  };
  // Announces `lat:<name>` from `el`. Returns false when the event was
  // cancelable and a listener called preventDefault(), which is how a
  // `before-` event stops the action it announces.
  lat.emit = function (el, name, detail, cancelable) {
    return (el || document).dispatchEvent(new CustomEvent('lat:' + name, {
      bubbles: true,
      cancelable: !!cancelable,
      detail: detail || {}
    }));
  };
  // Listens for `lat:<name>` anywhere on the page, including content that
  // arrives with a swap. The handler gets the detail, then the event. Returns
  // the function that stops listening.
  lat.on = function (name, handler) {
    function listener(e) { handler(e.detail, e); }
    document.addEventListener('lat:' + name, listener);
    return function () { document.removeEventListener('lat:' + name, listener); };
  };
  lat.assets = {
    // Idempotently load a stylesheet or script (for fragments whose assets are
    // not already on the page).
    ensure: function (url) {
      if (document.querySelector('[data-lat-asset="' + url + '"]')) return;
      var el;
      if (/\.css(\?|$)/.test(url)) {
        el = document.createElement('link');
        el.rel = 'stylesheet';
        el.href = url;
      } else {
        el = document.createElement('script');
        el.src = url;
        el.defer = true;
      }
      el.setAttribute('data-lat-asset', url);
      document.head.appendChild(el);
    }
  };
  var unstarted = '[data-lat-widget]:not([data-lat-ready])';
  function start(el) {
    var init = registry[el.getAttribute('data-lat-widget')];
    if (!init) return;
    el.setAttribute('data-lat-ready', '1');
    var controller = init(el);
    if (!controller) return;
    controllers.set(el, controller);
    // An island that builds markup around its element names that markup as
    // `root`, so `lat.get` finds it from inside the markup too.
    if (controller.root && controller.root !== el) {
      controller.root.setAttribute('data-lat-widget', el.getAttribute('data-lat-widget'));
      controller.root.setAttribute('data-lat-ready', '1');
      controllers.set(controller.root, controller);
    }
  }
  // Starts every island in `root` that has not started, `root` included: a
  // swapped-in fragment is often the island itself.
  function scan(root) {
    var scope = root && root.querySelectorAll ? root : document;
    if (scope.matches && scope.matches(unstarted)) start(scope);
    scope.querySelectorAll(unstarted).forEach(start);
  }
  lat.scan = scan;
  document.addEventListener('DOMContentLoaded', function () {
    scan(document);
    var e = document.getElementById('lat-mode-ico');
    if (e) e.textContent = latModeGlyph(latMode());
  });
  document.addEventListener('htmx:load', function (ev) { scan(ev.target); });
})();

// Raises a toast from script, matching the server-rendered flash markup so both
// look and dismiss the same.
window.lat.flash = function (text, level) {
  var box = document.querySelector('.lat-flashes');
  if (!box) {
    box = document.createElement('div');
    box.className = 'lat-flashes';
    box.setAttribute('role', 'status');
    box.setAttribute('aria-live', 'polite');
    document.body.insertBefore(box, document.body.firstChild);
  }
  var toast = document.createElement('div');
  toast.className = 'lat-flash is-' + (level || 'error');
  toast.setAttribute('data-lat-widget', 'flash');
  var label = document.createElement('span');
  label.className = 'lat-flash__text';
  label.textContent = text;
  var close = document.createElement('button');
  close.type = 'button';
  close.className = 'lat-flash__close';
  close.innerHTML = '&times;';
  close.addEventListener('click', function () { latDismissFlash(close); });
  toast.appendChild(label);
  toast.appendChild(close);
  box.appendChild(toast);
  // Started as the island a server-rendered message is, so it announces itself
  // and leaves on the same terms.
  window.lat.scan(toast);
};

// A response htmx will not swap is otherwise swallowed, so a 500 or a dropped
// connection leaves the click with no outcome. The 422 above marks itself
// not-an-error, so a form's own validation errors never reach this.
function latRequestFailed() {
  window.lat.flash(document.body.getAttribute('data-lat-request-error') || 'Request failed.', 'error');
}
document.addEventListener('htmx:responseError', latRequestFailed);
document.addEventListener('htmx:sendError', latRequestFailed);

// Top progress bar: shown while any htmx request is in flight. The bar creeps
// toward the right while waiting, since the real duration is unknown.
(function () {
  var inflight = 0;
  var bar = null;
  var timer = null;
  function element() {
    if (!bar) {
      bar = document.createElement('div');
      bar.className = 'lat-progress';
      document.body.appendChild(bar);
    }
    return bar;
  }
  document.addEventListener('htmx:beforeRequest', function () {
    inflight++;
    clearTimeout(timer);
    var b = element();
    b.classList.remove('is-done');
    b.classList.add('is-active');
  });
  document.addEventListener('htmx:afterRequest', function () {
    inflight = Math.max(0, inflight - 1);
    if (inflight > 0) return;
    var b = element();
    b.classList.add('is-done');
    timer = setTimeout(function () { b.classList.remove('is-active', 'is-done'); }, 220);
  });
})();

// Confirm dialog: a control carrying data-lat-confirm asks before it acts. A
// modal rather than window.confirm, because a native dialog blocks the page and
// cannot be styled or localized with the rest of the admin.
(function () {
  var pending = null;
  // The callback of a question asked from script, while it is open.
  var asked = null;

  function box() {
    var el = document.getElementById('lat-confirm');
    if (el) return el;
    el = document.createElement('div');
    el.id = 'lat-confirm';
    el.className = 'lat-modal';
    el.setAttribute('role', 'dialog');
    el.setAttribute('aria-modal', 'true');
    el.innerHTML =
      '<div class="lat-modal__backdrop" data-lat-close></div>' +
      '<div class="lat-modal__panel">' +
      '<p class="lat-modal__text"></p>' +
      '<div class="lat-modal__actions">' +
      '<button type="button" class="lat-btn lat-btn--ghost" data-lat-close></button>' +
      '<button type="button" class="lat-btn lat-btn--danger" data-lat-go></button>' +
      '</div></div>';
    document.body.appendChild(el);
    el.addEventListener('click', function (e) {
      if (e.target.hasAttribute('data-lat-close')) close();
      if (e.target.hasAttribute('data-lat-go')) go();
    });
    return el;
  }

  function close() {
    if (answer(false)) return;
    if (pending) window.lat.emit(pending, 'confirm:cancelled', {});
    hide();
  }

  function hide() {
    pending = null;
    var el = document.getElementById('lat-confirm');
    if (el) el.classList.remove('is-open');
  }

  // Asks from script, with no control behind the question: `done` hears true
  // or false. `labels.go` and `labels.cancel` name the two buttons, and
  // `labels.focus: 'cancel'` puts the cursor on the one that changes nothing.
  window.lat.confirm = function (text, labels, done) {
    labels = labels || {};
    asked = done;
    pending = null;
    var modal = box();
    modal.querySelector('.lat-modal__text').textContent = text;
    modal.querySelector('[data-lat-go]').textContent = labels.go || 'OK';
    modal.querySelector('[data-lat-close]:not(.lat-modal__backdrop)').textContent =
      labels.cancel || document.body.getAttribute('data-lat-cancel') || 'Cancel';
    modal.classList.add('is-open');
    modal.querySelector(
      labels.focus === 'cancel' ? '.lat-modal__actions [data-lat-close]' : '[data-lat-go]'
    ).focus();
    window.lat.emit(document, 'confirm:opened', { text: text });
  };

  function answer(yes) {
    var done = asked;
    asked = null;
    if (!done) return false;
    hide();
    window.lat.emit(document, yes ? 'confirm:confirmed' : 'confirm:cancelled', {});
    done(yes);
    return true;
  }

  function go() {
    if (answer(true)) return;
    var el = pending;
    hide();
    if (!el) return;
    window.lat.emit(el, 'confirm:confirmed', {});
    // Marked so the second pass through the handler lets it through.
    el.setAttribute('data-lat-confirmed', '1');
    el.click();
    el.removeAttribute('data-lat-confirmed');
  }

  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') close();
  });

  // Capture phase, so the click never reaches htmx or the form until confirmed.
  document.addEventListener('click', function (e) {
    var el = e.target.closest ? e.target.closest('[data-lat-confirm]') : null;
    if (!el || el.hasAttribute('data-lat-confirmed')) return;
    e.preventDefault();
    e.stopPropagation();
    pending = el;
    var modal = box();
    modal.querySelector('.lat-modal__text').textContent = el.getAttribute('data-lat-confirm');
    modal.querySelector('[data-lat-go]').textContent = el.textContent.trim() || 'OK';
    modal.querySelector('[data-lat-close]:not(.lat-modal__backdrop)').textContent =
      document.body.getAttribute('data-lat-cancel') || 'Cancel';
    modal.classList.add('is-open');
    modal.querySelector('[data-lat-go]').focus();
    window.lat.emit(el, 'confirm:opened', { text: el.getAttribute('data-lat-confirm') });
  }, true);
})();

// Dialogs: a control carrying data-lat-dialog opens the <dialog> it names,
// one carrying data-lat-dialog-close closes the dialog it sits in, and a click
// on the backdrop closes too. Native, so Escape and focus containment need no
// code here. `data-lat-dialog-set="field=value"` fills a field before opening,
// so one dialog serves several openers (a New button per group).
(function () {
  document.addEventListener('click', function (e) {
    if (!e.target.closest) return;
    var opener = e.target.closest('[data-lat-dialog]');
    if (opener) {
      var dialog = document.getElementById(opener.getAttribute('data-lat-dialog'));
      if (!dialog || typeof dialog.showModal !== 'function') return;
      e.preventDefault();
      (opener.getAttribute('data-lat-dialog-set') || '').split(';').forEach(function (pair) {
        var at = pair.indexOf('=');
        if (at < 1) return;
        var field = dialog.querySelector('[name="' + pair.slice(0, at).trim() + '"]');
        if (field) field.value = pair.slice(at + 1).trim();
      });
      dialog.showModal();
      var first = dialog.querySelector('[autofocus], input:not([type=hidden]):not([readonly]), select, textarea');
      if (first) first.focus();
      window.lat.emit(dialog, 'dialog:opened', {});
      return;
    }
    var closer = e.target.closest('[data-lat-dialog-close]');
    if (closer) {
      var open = closer.closest('dialog');
      if (open) open.close();
      return;
    }
    // With no padding of its own, a click that lands on the dialog element
    // itself landed on the backdrop.
    if (e.target.matches && e.target.matches('dialog.lat-dialog')) e.target.close();
  });
})();

// The uploader: a drop zone over a file input. Given data-upload-url it sends
// each picked or dropped file itself, one request per file, with the request
// token first in the body as the upload guard requires and data-upload-fields
// ("folder=3") before the file; a row per file shows progress and the outcome,
// and the page reloads once every file is stored. Without the URL it is the
// styled field of an ordinary form: it names what was picked and the form
// submits as it always did.
window.lat.widget('upload', function (root) {
  var input = root.querySelector('input[type=file]');
  var list = root.querySelector('.lat-upload__list');
  var url = root.getAttribute('data-upload-url');
  var form = root.closest('form');
  if (!input || !list) return;
  var words = {
    uploading: root.getAttribute('data-lat-uploading') || 'Uploading',
    stored: root.getAttribute('data-lat-stored') || 'Stored',
    failed: root.getAttribute('data-lat-failed') || 'Could not be stored',
  };
  var pending = 0;
  var failures = 0;

  function size(n) {
    if (n < 1024) return n + ' B';
    if (n < 1048576) return (n / 1024).toFixed(0) + ' KB';
    return (n / 1048576).toFixed(1) + ' MB';
  }

  function row(file) {
    var li = document.createElement('li');
    li.className = 'lat-upload__item';
    li.innerHTML =
      '<span class="lat-upload__name"></span><span class="lat-upload__size"></span>' +
      (url ? '<span class="lat-upload__bar"><i></i></span><span class="lat-upload__status"></span>' : '');
    li.querySelector('.lat-upload__name').textContent = file.name;
    li.querySelector('.lat-upload__size').textContent = size(file.size);
    list.appendChild(li);
    return li;
  }

  function finish(li, ok, text) {
    li.classList.add(ok ? 'is-done' : 'is-failed');
    li.querySelector('.lat-upload__status').textContent = text;
    if (!ok) failures += 1;
    pending -= 1;
    window.lat.emit(root, ok ? 'upload:stored' : 'upload:failed', { name: li.querySelector('.lat-upload__name').textContent, reason: text });
    if (pending === 0) {
      window.lat.emit(root, 'upload:done', { failed: failures });
      if (failures === 0) setTimeout(function () { location.reload(); }, 350);
    }
  }

  function send(file, li) {
    var body = new FormData();
    var token = form && form.querySelector('input[name="_csrf"]');
    if (token) body.append('_csrf', token.value);
    (root.getAttribute('data-upload-fields') || '').split(';').forEach(function (pair) {
      var at = pair.indexOf('=');
      if (at > 0) body.append(pair.slice(0, at).trim(), pair.slice(at + 1).trim());
    });
    body.append(input.name || 'file', file, file.name);
    var xhr = new XMLHttpRequest();
    var bar = li.querySelector('.lat-upload__bar i');
    var status = li.querySelector('.lat-upload__status');
    status.textContent = words.uploading;
    xhr.upload.addEventListener('progress', function (e) {
      if (e.lengthComputable) bar.style.width = Math.round((e.loaded / e.total) * 100) + '%';
    });
    xhr.addEventListener('load', function () {
      var out = null;
      try { out = JSON.parse(xhr.responseText); } catch (e) { out = null; }
      var stored = out && out.stored && out.stored[0];
      var refused = out && out.refused && out.refused[0];
      if (xhr.status === 200 && stored) finish(li, true, words.stored);
      else finish(li, false, (refused && refused.reason) || words.failed);
    });
    xhr.addEventListener('error', function () { finish(li, false, words.failed); });
    xhr.open('POST', url);
    xhr.setRequestHeader('X-Requested-With', 'fetch');
    xhr.send(body);
  }

  function take(files) {
    var picked = Array.prototype.slice.call(files || []);
    if (!picked.length) return;
    if (!url) {
      list.innerHTML = '';
      picked.forEach(row);
      window.lat.emit(root, 'upload:picked', { count: picked.length });
      return;
    }
    if (!window.lat.emit(root, 'upload:before-send', { count: picked.length }, true)) return;
    picked.forEach(function (file) {
      pending += 1;
      send(file, row(file));
    });
  }

  if (url) {
    // The zone sends, so the form's own button and requirement step aside.
    input.required = false;
    var submit = form && form.querySelector('[data-upload-submit]');
    if (submit) submit.hidden = true;
  }
  input.addEventListener('change', function () {
    take(input.files);
    if (url) input.value = '';
  });
  ['dragenter', 'dragover'].forEach(function (name) {
    root.addEventListener(name, function (e) { e.preventDefault(); root.classList.add('is-dragging'); });
  });
  ['dragleave', 'drop'].forEach(function (name) {
    root.addEventListener(name, function (e) { e.preventDefault(); root.classList.remove('is-dragging'); });
  });
  root.addEventListener('drop', function (e) {
    var files = e.dataTransfer && e.dataTransfer.files;
    if (!files || !files.length) return;
    if (url) {
      take(files);
    } else {
      // Dropped into the field: the form submits them as if picked.
      try { input.files = files; } catch (err) { /* an older engine: pick instead */ }
      take(input.files);
    }
  });
  return { take: take };
});

// Dropdown dismissal. A <details> stays open until its own summary is clicked
// again, which is not what a dropdown is expected to do: choosing an item or
// clicking away should close it. Bound once at the document, so a menu that
// arrives with a swap is covered without re-binding.
//
// A menu that must survive a click declares data-lat-keep-open.
(function () {
  function closeAll(except) {
    document.querySelectorAll('details.lat-menu[open]').forEach(function (m) {
      if (m !== except && !m.hasAttribute('data-lat-keep-open')) m.open = false;
    });
  }

  document.addEventListener('click', function (e) {
    if (!e.target.closest) return;
    var menu = e.target.closest('details.lat-menu');
    // Inside a panel only a link ends the interaction. Ticking a box in the
    // column picker is the operator still choosing, so it must stay open.
    var chose = menu && e.target.closest('.lat-menu__panel') && e.target.closest('a,button');
    closeAll(chose ? null : menu);
  });

  // Escape closes the open menu and hands focus back to what opened it, which
  // is where the keyboard was before.
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape') return;
    var open = document.querySelector('details.lat-menu[open]');
    if (!open || open.hasAttribute('data-lat-keep-open')) return;
    open.open = false;
    var summary = open.querySelector('summary');
    if (summary) summary.focus();
  });
})();

// Flash toasts: auto-dismiss non-error messages after a few seconds.
window.lat.widget('flash', function (el) {
  var level = (el.className.match(/is-(success|error|info)/) || [])[1] || 'info';
  window.lat.emit(el, 'flash:shown', { text: flashText(el), level: level });
  // Errors stay until dismissed, and so does anything marked to persist.
  if (el.classList.contains('is-error') || el.hasAttribute('data-lat-persist')) return;
  setTimeout(function () { latDismissFlash(el); }, 5000);
});

// Repeater: add and remove rows. Rows are renumbered after every change so the
// submitted indices read 0,1,2; the server sorts the indices it finds rather
// than counting, so this is tidiness, not correctness.
window.lat.widget('repeater', function (root) {
  var rows = root.querySelector('.lat-repeater__rows');
  var blank = root.querySelector('.lat-repeater__blank');
  var add = root.querySelector('.lat-repeater__add');
  if (!rows || !blank || !add) return;
  var reorder = root.getAttribute('data-lat-reorder') !== 'off';
  var duplicate = root.getAttribute('data-lat-duplicate') !== 'off';
  // The placeholder a row with no name shows, as the server wrote it.
  var untitled = blank.content.querySelector('.lat-repeater__untitled');
  untitled = untitled ? untitled.textContent : 'Untitled';

  function all() {
    return Array.prototype.slice.call(rows.querySelectorAll(':scope > .lat-repeater__row'));
  }

  // Indices read 0,1,2 after every change, and the first row cannot move up
  // nor the last down.
  function renumber() {
    var list = all();
    list.forEach(function (row, index) {
      row.querySelectorAll('[name]').forEach(function (control) {
        control.name = control.name.replace(/\[[^\]]*\]/, '[' + index + ']');
      });
      var up = row.querySelector('.lat-repeater__up');
      var down = row.querySelector('.lat-repeater__down');
      if (up) up.disabled = index === 0;
      if (down) down.disabled = index === list.length - 1;
    });
  }

  // The line that names a collapsed row, kept current as the operator types.
  // Without this a new row would read as untitled until the page reloaded.
  var summaryIndex = parseInt(add.getAttribute('data-lat-summary-index'), 10) || 0;

  function retitle(row) {
    var title = row.querySelector('.lat-repeater__title');
    if (!title) return;
    var control = row.querySelectorAll('.lat-repeater__fields [name]')[summaryIndex];
    var text = control ? String(control.value || '').split('\n')[0].trim() : '';
    if (text) {
      title.textContent = text;
    } else {
      title.textContent = '';
      var span = document.createElement('span');
      span.className = 'lat-repeater__untitled';
      span.textContent = untitled;
      title.appendChild(span);
    }
  }

  // The controls that need the script, shown once it runs.
  function reveal(scope) {
    scope.querySelectorAll('.lat-repeater__up, .lat-repeater__down, .lat-repeater__duplicate').forEach(function (b) {
      b.hidden = false;
    });
  }

  function fresh(after) {
    var row = blank.content.cloneNode(true).firstElementChild;
    if (after) after.insertAdjacentElement('afterend', row); else rows.appendChild(row);
    renumber();
    reveal(row);
    window.lat.scan(row);
    return row;
  }

  function addRow() {
    if (!window.lat.emit(root, 'repeater:before-add', { count: all().length }, true)) return null;
    var added = fresh(null);
    // A details row is added open, so focus lands where the operator is looking.
    var first = added.querySelector('.lat-repeater__fields [name], [name]');
    if (first) first.focus();
    var list = all();
    window.lat.emit(root, 'repeater:added', { row: added, index: list.length - 1, count: list.length });
    return added;
  }

  function removeRow(row) {
    var index = all().indexOf(row);
    if (index < 0) return false;
    if (!window.lat.emit(root, 'repeater:before-remove', { row: row, index: index, count: all().length }, true)) {
      return false;
    }
    row.remove();
    renumber();
    window.lat.emit(root, 'repeater:removed', { index: index, count: all().length });
    return true;
  }

  function moveRow(from, to) {
    var list = all();
    var row = list[from];
    var target = list[to];
    if (!row || !target || from === to) return false;
    if (to > from) target.insertAdjacentElement('afterend', row);
    else target.insertAdjacentElement('beforebegin', row);
    renumber();
    var focus = row.querySelector(to > from ? '.lat-repeater__down' : '.lat-repeater__up');
    if (focus && !focus.disabled) focus.focus();
    window.lat.emit(root, 'repeater:moved', { row: row, from: from, to: to, count: list.length });
    return true;
  }

  // Values by position: a fresh row has the same controls in the same order,
  // so a picker's hidden id and its box both copy across.
  function copyValues(from, to) {
    var sources = from.querySelectorAll('input, select, textarea');
    var targets = to.querySelectorAll('input, select, textarea');
    sources.forEach(function (source, i) {
      var target = targets[i];
      if (!target || target.tagName !== source.tagName || target.type !== source.type) return;
      if (source.type === 'checkbox' || source.type === 'radio') target.checked = source.checked;
      else target.value = source.value;
      target.dispatchEvent(new Event('input', { bubbles: true }));
      target.dispatchEvent(new Event('change', { bubbles: true }));
    });
    // A picker keeps its choice in its island as well as its inputs.
    var pickers = from.querySelectorAll('[data-lat-widget="ref-picker"]');
    to.querySelectorAll('[data-lat-widget="ref-picker"]').forEach(function (picker, i) {
      var origin = pickers[i] && window.lat.get(pickers[i]);
      var mine = window.lat.get(picker);
      if (origin && mine && origin.value()) mine.choose({ id: origin.value(), label: origin.label() });
    });
  }

  function duplicateRow(index) {
    var list = all();
    var source = list[index];
    if (!source) return null;
    if (!window.lat.emit(root, 'repeater:before-add', { count: list.length }, true)) return null;
    var row = fresh(source);
    copyValues(source, row);
    if (row.tagName === 'DETAILS') {
      retitle(row);
      row.open = true;
    }
    var first = row.querySelector('.lat-repeater__fields [name], [name]');
    if (first) first.focus();
    window.lat.emit(root, 'repeater:duplicated', { row: row, index: index + 1, count: all().length });
    return row;
  }

  add.addEventListener('click', addRow);

  root.addEventListener('input', function (e) {
    var row = e.target.closest('.lat-repeater__row');
    if (row && row.tagName === 'DETAILS') retitle(row);
  });

  root.addEventListener('click', function (e) {
    var button = e.target.closest('button');
    var row = button && button.closest('.lat-repeater__row');
    if (!row || !rows.contains(row)) return;
    var index = all().indexOf(row);
    if (button.classList.contains('lat-repeater__remove')) removeRow(row);
    else if (reorder && button.classList.contains('lat-repeater__up')) moveRow(index, index - 1);
    else if (reorder && button.classList.contains('lat-repeater__down')) moveRow(index, index + 1);
    else if (duplicate && button.classList.contains('lat-repeater__duplicate')) duplicateRow(index);
    else return;
    // A button in a summary must not toggle the row.
    e.preventDefault();
  });

  if (reorder || duplicate) reveal(rows);
  renumber();

  return {
    add: addRow,
    remove: function (index) {
      var row = all()[index];
      return row ? removeRow(row) : false;
    },
    move: moveRow,
    duplicate: duplicateRow,
    count: function () { return all().length; }
  };
});

// Checklist: boxes in groups. The markup submits and the groups open and close
// without this; the island adds what needs a script. A group's box is ticked
// when all of its boxes are, clear when none is and in between otherwise, and
// ticking it ticks the group. Counts follow the ticks. Select all, select none
// and a group's box act on what the search is showing, so narrowing the list
// and ticking what is left is two steps. Every change is announced once as
// `checklist:changed`, with the ticked values.
window.lat.widget('checklist', function (root) {
  var pattern = root.getAttribute('data-lat-count') || '{n} / {total}';
  var bar = root.querySelector('.lat-checklist__bar');
  var search = root.querySelector('[data-lat-checklist-search]');
  var total = root.querySelector('[data-lat-checklist-total]');
  var empty = root.querySelector('.lat-checklist__empty');

  function list(scope, selector) {
    return Array.prototype.slice.call(scope.querySelectorAll(selector));
  }
  function boxes(scope) { return list(scope || root, '.lat-checklist__item input[type="checkbox"]'); }
  function groups() { return list(root, '.lat-checklist__group'); }
  function shown(box) { return !box.closest('.lat-checklist__item').hidden; }
  function own(group, selector) {
    var summary = group.firstElementChild;
    return summary ? summary.querySelector(selector) : null;
  }
  function ticked(all) { return all.filter(function (b) { return b.checked; }); }
  function values() { return ticked(boxes()).map(function (b) { return b.value; }); }
  function count(n, of) { return pattern.replace('{n}', n).replace('{total}', of); }

  function refresh() {
    groups().forEach(function (group) {
      var all = boxes(group);
      var on = ticked(all).length;
      var parent = own(group, '.lat-checklist__parent');
      var label = own(group, '.lat-checklist__count');
      if (parent) {
        parent.checked = all.length > 0 && on === all.length;
        parent.indeterminate = on > 0 && on < all.length;
        parent.disabled = !all.some(function (b) { return !b.disabled; });
      }
      if (label) label.textContent = count(on, all.length);
    });
    if (total) total.textContent = count(ticked(boxes()).length, boxes().length);
  }

  function changed() {
    refresh();
    var on = values();
    window.lat.emit(root, 'checklist:changed', { values: on, count: on.length, total: boxes().length });
  }

  // Ticks or clears every box that may change. True when any did.
  function put(all, on) {
    var moved = false;
    all.forEach(function (box) {
      if (box.disabled || box.checked === on) return;
      box.checked = on;
      moved = true;
    });
    return moved;
  }

  function filter(text) {
    var query = (text || '').trim().toLowerCase();
    boxes().forEach(function (box) {
      var item = box.closest('.lat-checklist__item');
      item.hidden = query !== '' && item.textContent.toLowerCase().indexOf(query) === -1;
    });
    var any = false;
    groups().forEach(function (group) {
      var matches = boxes(group).some(shown);
      any = any || matches;
      group.hidden = !matches;
      if (query === '') {
        // Back to how the group stood before the search opened it.
        if (group.hasAttribute('data-lat-was')) {
          group.open = group.getAttribute('data-lat-was') === 'open';
          group.removeAttribute('data-lat-was');
        }
      } else {
        if (!group.hasAttribute('data-lat-was')) {
          group.setAttribute('data-lat-was', group.open ? 'open' : 'closed');
        }
        group.open = matches;
      }
    });
    any = any || boxes().some(shown);
    if (empty) empty.hidden = any || boxes().length === 0;
  }

  root.addEventListener('change', function (e) {
    var target = e.target;
    if (target.classList.contains('lat-checklist__parent')) {
      put(boxes(target.closest('.lat-checklist__group')).filter(shown), target.checked);
      changed();
    } else if (target.matches('.lat-checklist__item input[type="checkbox"]')) {
      changed();
    }
  });

  root.addEventListener('click', function (e) {
    var all = e.target.closest('[data-lat-checklist-all]');
    var none = e.target.closest('[data-lat-checklist-none]');
    if (!all && !none) return;
    if (put(boxes().filter(shown), !!all)) changed();
  });

  if (search) {
    search.addEventListener('input', function () { filter(search.value); });
    // Escape clears the search first, and only then reaches anything outside.
    search.addEventListener('keydown', function (e) {
      if (e.key !== 'Escape' || search.value === '') return;
      e.stopPropagation();
      search.value = '';
      filter('');
    });
  }

  list(root, '.lat-checklist__parent').forEach(function (parent) { parent.hidden = false; });
  if (bar) bar.hidden = false;
  refresh();

  return {
    values: values,
    count: function () { return values().length; },
    // Ticks exactly `wanted`, leaving locked boxes as they are.
    set: function (wanted) {
      var moved = false;
      boxes().forEach(function (box) {
        var on = wanted.indexOf(box.value) !== -1;
        if (box.disabled || box.checked === on) return;
        box.checked = on;
        moved = true;
      });
      if (moved) changed();
    },
    all: function () { if (put(boxes(), true)) changed(); },
    none: function () { if (put(boxes(), false)) changed(); },
    search: function (text) {
      if (search) search.value = text || '';
      filter(text);
    }
  };
});

// Select-all checkbox in a list header: ticks every row box in its table. Bound
// by structure, and re-bound after a swap because the header comes back with it.
window.lat.widget('pick-all', function (box) {
  var table = box.closest('table');
  if (!table) return;
  box.addEventListener('change', function () {
    table.querySelectorAll('tbody input[type="checkbox"][name="id"]').forEach(function (row) {
      row.checked = box.checked;
    });
    latSelectionChanged(table);
  });
});

// A click on a row box with Shift held ticks, or clears, every box between it
// and the box clicked before it, to the state of the one just clicked. The
// click runs before the change the browser fires for the clicked box, so that
// change announces the whole range once.
document.addEventListener('click', function (e) {
  var box = e.target;
  if (!box.matches || !box.matches('tbody input[type="checkbox"][name="id"]')) return;
  var table = box.closest('table');
  if (!table) return;
  var all = Array.prototype.slice.call(table.querySelectorAll('tbody input[type="checkbox"][name="id"]'));
  var last = table._latLastPick;
  var from = all.indexOf(last);
  if (e.shiftKey && from !== -1 && last !== box) {
    var to = all.indexOf(box);
    all.slice(Math.min(from, to), Math.max(from, to) + 1).forEach(function (other) {
      other.checked = box.checked;
    });
  }
  table._latLastPick = box;
});

// The rows ticked in a list, announced whenever they change.
function latSelectionChanged(table) {
  var ids = Array.prototype.map.call(
    table.querySelectorAll('tbody input[type="checkbox"][name="id"]:checked'),
    function (box) { return box.value; }
  );
  window.lat.emit(table, 'selection:changed', { ids: ids, count: ids.length });
}
document.addEventListener('change', function (e) {
  if (!e.target.matches || !e.target.matches('tbody input[type="checkbox"][name="id"]')) return;
  var table = e.target.closest('table');
  if (table) latSelectionChanged(table);
});

// Bulk Delete: it sits in the action row, outside the table it acts on, so no
// markup can tell it whether anything is selected. It starts disabled and
// follows the row checkboxes, which come back fresh with every swap.
window.lat.widget('bulk-delete', function (btn) {
  function sync() {
    btn.disabled = !document.querySelector('input[type="checkbox"][name="id"]:checked');
  }
  // The select-all box ticks rows without firing their change events, so listen
  // for it too rather than for the rows alone.
  document.addEventListener('change', function (e) {
    if (e.target.matches('input[name="id"], [data-lat-widget="pick-all"]')) sync();
  });
  document.addEventListener('htmx:load', sync);
  sync();
});

// Copy button: copies its input group's value, with brief confirmation. Binds by
// structure (closest group), so it survives repeater path-ids.
window.lat.widget('copy', function (btn) {
  var label = btn.textContent;
  btn.addEventListener('click', function () {
    var group = btn.closest('.lat-input-group');
    var input = group && group.querySelector('input');
    if (!input || !navigator.clipboard) return;
    navigator.clipboard.writeText(input.value).then(function () {
      window.lat.emit(btn, 'copy:copied', { value: input.value });
      btn.classList.add('is-copied');
      btn.textContent = 'Copied';
      setTimeout(function () {
        btn.classList.remove('is-copied');
        btn.textContent = label;
      }, 1200);
    });
  });
});

// Enter policy. One rule per scope, framework-wide, so every form behaves the
// way people expect without each screen wiring it:
//   * a single-line field: Enter submits (the browser's implicit submission;
//     every rendered form carries a submit button, which that needs);
//   * a textarea: Enter is a newline, Cmd/Ctrl+Enter submits;
//   * inside `[data-lat-enter-scope]` (a picker, a repeater): Enter acts there
//     and never submits the form around it;
//   * `data-lat-enter="off"` on a form or a field stops Enter submitting;
//     `data-lat-enter="next"` on a field moves focus to the next one.
(function () {
  var NOT_TEXT = /^(checkbox|radio|file|button|submit|reset|image|range|color)$/;
  function isSingleLine(el) {
    return el.tagName === 'INPUT' && !NOT_TEXT.test(el.type);
  }
  function rule(el) {
    var carrier = el.closest('[data-lat-enter]');
    return carrier ? carrier.getAttribute('data-lat-enter') : 'submit';
  }
  function focusables(root) {
    return Array.prototype.filter.call(
      root.querySelectorAll('input, select, textarea, button, [tabindex]'),
      function (f) { return !f.disabled && f.type !== 'hidden' && f.offsetParent !== null; }
    );
  }
  function focusNext(el, root) {
    var list = focusables(root);
    var i = list.indexOf(el);
    if (i < 0 || i + 1 >= list.length) return false;
    list[i + 1].focus();
    return true;
  }
  // A repeater: the next field in the row, or a new row from the last one.
  function withinRows(el, scope) {
    var row = el.closest('.lat-repeater__row');
    if (row && focusNext(el, row)) return;
    var add = scope.querySelector(':scope > .lat-repeater__add');
    if (add) add.click();
  }
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Enter' || e.isComposing || e.defaultPrevented) return;
    var el = e.target;
    if (!el || !el.closest) return;
    var form = el.form || el.closest('form');
    if (e.metaKey || e.ctrlKey) {
      if (!form) return;
      e.preventDefault();
      form.requestSubmit();
      return;
    }
    if (!isSingleLine(el)) return;
    var scope = el.closest('[data-lat-enter-scope]');
    if (scope && form && form.contains(scope)) {
      e.preventDefault();
      var kind = scope.getAttribute('data-lat-enter-scope');
      var target = scope.getAttribute('data-lat-enter-target');
      if (kind === 'rows') withinRows(el, scope);
      else if (target) { var t = scope.querySelector(target); if (t) t.click(); }
      return;
    }
    var what = rule(el);
    if (what === 'off') e.preventDefault();
    else if (what === 'next') { e.preventDefault(); focusNext(el, form || document); }
  }, true);
})();

// Form: two things every content form does. It asks before the operator leaves
// with changes unsaved, and it puts the cursor where they will type next: the
// first field of a new record, or the first field a save refused.
//
// Changed means different from how the form opened, so typing and undoing it
// is not a change. A form that came back refused (`data-lat-refused`) opens
// changed, since what it holds was never saved.
// `data-lat-confirm-leave="off"` stops the question;
// `data-lat-focus="off"` leaves the cursor alone.
(function () {
  var guarded = [];
  var leaving = false;

  function snapshot(form) {
    var parts = [];
    new FormData(form).forEach(function (value, key) {
      if (key === '_csrf') return;
      parts.push(key + '=' + (typeof value === 'string' ? value : value.name));
    });
    return parts.join('&');
  }

  function changed() {
    return guarded.some(function (g) {
      return document.body.contains(g.form) && !g.sent && (g.refused || snapshot(g.form) !== g.opened);
    });
  }

  window.lat.widget('form', function (form) {
    var refused = form.hasAttribute('data-lat-refused');
    var state = { form: form, opened: snapshot(form), refused: refused, sent: false };
    if (form.getAttribute('data-lat-confirm-leave') !== 'off') {
      guarded = guarded.filter(function (g) { return document.body.contains(g.form); });
      guarded.push(state);
    }
    form.addEventListener('submit', function () { state.sent = true; });
    // A request that never arrived leaves the form as unsaved as it was.
    form.addEventListener('htmx:sendError', function () { state.sent = false; });
    form.addEventListener('htmx:responseError', function () { state.sent = false; });

    var focus = form.getAttribute('data-lat-focus');
    if (focus !== 'off') {
      var field = refused ? form.querySelector('.lat-field__error') : null;
      var scope = field ? field.closest('.lat-field') : (refused || focus === 'first' ? form : null);
      var control = scope && scope.querySelector(
        'input:not([type="hidden"]):not([disabled]):not([readonly]), select:not([disabled]), textarea:not([disabled]):not([readonly])'
      );
      if (control) {
        control.focus({ preventScroll: !refused });
        if (refused && control.scrollIntoView) control.scrollIntoView({ block: 'center' });
      }
    }

    return {
      changed: function () { return !state.sent && (state.refused || snapshot(form) !== state.opened); },
      // Takes the form as it stands for saved, so leaving asks nothing.
      settle: function () { state.refused = false; state.opened = snapshot(form); }
    };
  });

  // Closing the tab, reloading and the back button: only the browser can ask.
  window.addEventListener('beforeunload', function (e) {
    if (leaving || !changed()) return;
    e.preventDefault();
    e.returnValue = '';
  });

  // A link inside the admin: the admin asks, in its own dialog.
  document.addEventListener('click', function (e) {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    var link = e.target.closest ? e.target.closest('a[href]') : null;
    if (!link || link.target === '_blank' || link.hasAttribute('download')) return;
    var href = link.getAttribute('href');
    if (!href || href.charAt(0) === '#' || /^javascript:/i.test(href)) return;
    if (!changed()) return;
    e.preventDefault();
    var body = document.body;
    window.lat.confirm(
      body.getAttribute('data-lat-leave') || 'You have unsaved changes. Leave without saving?',
      {
        go: body.getAttribute('data-lat-leave-go') || 'Leave',
        cancel: body.getAttribute('data-lat-leave-stay') || 'Stay',
        // Enter on a question about losing work should keep the work.
        focus: 'cancel'
      },
      function (yes) {
        if (!yes) return;
        leaving = true;
        window.location.href = link.href;
      }
    );
  });
})();

// Counter: a field a rule limits shows how much of the limit is used. With
// `data-lat-counter="auto"` it appears once four fifths are used, which is when
// the limit starts to matter; with `on` it is always there.
(function () {
  function counter(control) {
    var next = control.closest('.lat-input-group') || control;
    var el = next.nextElementSibling;
    if (el && el.classList.contains('lat-counter')) return el;
    el = document.createElement('p');
    el.className = 'lat-counter';
    el.setAttribute('aria-live', 'polite');
    next.parentNode.insertBefore(el, next.nextSibling);
    return el;
  }
  function update(control) {
    var max = parseInt(control.getAttribute('maxlength'), 10);
    if (!max) return;
    var used = control.value.length;
    var el = counter(control);
    el.textContent = used + ' / ' + max;
    el.classList.toggle('is-full', used >= max);
    el.hidden = control.getAttribute('data-lat-counter') === 'auto' && used < max * 0.8;
  }
  function all(root) {
    var scope = root && root.querySelectorAll ? root : document;
    scope.querySelectorAll('[data-lat-counter]').forEach(update);
  }
  document.addEventListener('input', function (e) {
    if (e.target.hasAttribute && e.target.hasAttribute('data-lat-counter')) update(e.target);
  });
  document.addEventListener('DOMContentLoaded', function () { all(document); });
  document.addEventListener('htmx:load', function (e) { all(e.target); });
})();

// Textarea: grows with what is typed, up to the height the stylesheet allows,
// so nothing is read through a slot. The stylesheet does this alone where the
// browser can; this is for the ones that cannot. `data-lat-grow="off"` keeps
// the height it was given.
(function () {
  if (window.CSS && CSS.supports && CSS.supports('field-sizing', 'content')) return;
  function fit(area) {
    if (area.getAttribute('data-lat-grow') === 'off') return;
    area.style.height = 'auto';
    area.style.height = (area.scrollHeight + 2) + 'px';
  }
  function all(root) {
    var scope = root && root.querySelectorAll ? root : document;
    scope.querySelectorAll('textarea.lat-input').forEach(fit);
  }
  document.addEventListener('input', function (e) {
    if (e.target.matches && e.target.matches('textarea.lat-input')) fit(e.target);
  });
  document.addEventListener('DOMContentLoaded', function () { all(document); });
  document.addEventListener('htmx:load', function (e) { all(e.target); });
})();

// Reveal: shows what was typed into a password field, and hides it again. The
// button starts hidden, so a page with no script has no button that does
// nothing. What is shown is hidden again when the form is sent.
window.lat.widget('reveal', function (btn) {
  var group = btn.closest('.lat-input-group');
  var input = group && group.querySelector('input');
  if (!input) return;
  function show(on) {
    input.type = on ? 'text' : 'password';
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
    window.lat.emit(btn, on ? 'reveal:shown' : 'reveal:hidden', {});
  }
  btn.hidden = false;
  btn.addEventListener('click', function () { show(input.type === 'password'); });
  if (input.form) {
    input.form.addEventListener('submit', function () {
      if (input.type !== 'password') show(false);
    });
  }
  return { show: function () { show(true); }, hide: function () { show(false); } };
});

// Preset: a field follows another as it is typed, shaped on the way (a slug
// from a title), until the operator edits it. Clearing it hands it back to
// the source. The wrapper carries `data-lat-preset="<field>"` and
// `data-lat-preset-type="exact|slug|url|file"`.
(function () {
  var LETTERS = { 'ß': 'ss', 'æ': 'ae', 'ø': 'o', 'œ': 'oe', 'đ': 'd', 'ł': 'l', 'þ': 'th' };
  function slug(text) {
    return text
      .toLowerCase()
      .replace(/[ßæøœđłþ]/g, function (c) { return LETTERS[c]; })
      .normalize('NFD').replace(/[\u0300-\u036f]/g, '')
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '');
  }
  function shape(text, kind) {
    if (text.trim() === '') return '';
    if (kind === 'exact') return text;
    if (kind === 'url') return '/' + slug(text);
    if (kind === 'file') return text.trim().replace(/\s+/g, '-');
    return slug(text);
  }
  function control(wrapper) {
    return wrapper.querySelector('input:not([type="hidden"]), textarea');
  }
  function followers(form, name) {
    return Array.prototype.filter.call(form.querySelectorAll('[data-lat-preset]'), function (w) {
      return w.getAttribute('data-lat-preset') === name;
    });
  }
  document.addEventListener('input', function (e) {
    var source = e.target;
    var form = source.form;
    if (!form || !source.name) return;
    // Editing a follower takes it over; emptying it hands it back.
    var own = source.closest('[data-lat-preset]');
    if (own && control(own) === source && !source._latPreset) {
      own._latTaken = source.value !== '';
    }
    followers(form, source.name).forEach(function (wrapper) {
      var target = control(wrapper);
      if (!target || wrapper._latTaken) return;
      var value = shape(source.value, wrapper.getAttribute('data-lat-preset-type'));
      if (target.value === value) return;
      target.value = value;
      target._latPreset = true;
      target.dispatchEvent(new Event('input', { bubbles: true }));
      target._latPreset = false;
      window.lat.emit(target, 'preset:filled', { from: source.name, value: value });
    });
  });
  // A follower that opens with a value of its own is already taken.
  function settle(root) {
    var scope = root && root.querySelectorAll ? root : document;
    scope.querySelectorAll('[data-lat-preset]').forEach(function (wrapper) {
      var target = control(wrapper);
      if (target && target.value !== '') wrapper._latTaken = true;
    });
  }
  document.addEventListener('DOMContentLoaded', function () { settle(document); });
  document.addEventListener('htmx:load', function (e) { settle(e.target); });
})();

// Trigger: a field changes when another field's state meets a condition. The
// wrapper carries `data-lat-trigger-action` (`show`, `hide`, `enable`,
// `disable`, `empty`, `fill[value]`, joined by `|`), `data-lat-trigger-field`
// (`name`, or `name[]` for every value of a checklist) and
// `data-lat-trigger-condition` (`checked`, `unchecked`, `value[..]`).
(function () {
  function values(form, field) {
    var many = /\[\]$/.test(field);
    var name = field.replace(/\[\]$/, '');
    var out = [];
    var controls = form.querySelectorAll('[name]');
    Array.prototype.forEach.call(controls, function (c) {
      var mine = many ? (c.name === name || c.name.indexOf(name + '[') === 0) : c.name === name;
      if (!mine) return;
      if (c.type === 'checkbox' || c.type === 'radio') {
        if (c.checked) out.push(c.value);
      } else if (c.tagName === 'SELECT' && c.multiple) {
        Array.prototype.forEach.call(c.selectedOptions, function (o) { out.push(o.value); });
      } else {
        out.push(c.value);
      }
    });
    return out;
  }
  function ticked(form, field) {
    var name = field.replace(/\[\]$/, '');
    return Array.prototype.some.call(form.querySelectorAll('[name]'), function (c) {
      return (c.type === 'checkbox' || c.type === 'radio') && (c.name === name || c.name.indexOf(name + '[') === 0) && c.checked;
    });
  }
  // `value[a][b*]` lists what counts; `value[]` is empty and `value[*]` anything.
  function wanted(condition) {
    var list = [];
    var re = /\[([^\]]*)\]/g, m;
    while ((m = re.exec(condition))) list.push(m[1]);
    return list;
  }
  function matches(want, have) {
    if (want === '*') return have !== '';
    if (want.indexOf('*') === -1) return want === have;
    var head = want.split('*')[0], tail = want.split('*').slice(1).join('*');
    return have.indexOf(head) === 0 && (tail === '' || have.slice(-tail.length) === tail);
  }
  function met(form, wrapper) {
    var field = wrapper.getAttribute('data-lat-trigger-field');
    var condition = wrapper.getAttribute('data-lat-trigger-condition');
    if (condition === 'checked') return ticked(form, field);
    if (condition === 'unchecked') return !ticked(form, field);
    var have = values(form, field);
    var want = wanted(condition);
    if (want.length === 0 || (want.length === 1 && want[0] === '')) return have.every(function (v) { return v === ''; });
    return have.some(function (v) { return want.some(function (w) { return matches(w, v); }); });
  }
  // A hidden field is left out of the submission, so a required one cannot
  // hold the form back from behind its own trigger.
  function conceal(wrapper, controls, hidden) {
    wrapper.hidden = hidden;
    controls.forEach(function (c) {
      if (hidden && !c.disabled) { c.disabled = true; c._latConcealed = true; }
      else if (!hidden && c._latConcealed) { c.disabled = false; c._latConcealed = false; }
    });
  }
  function apply(form, wrapper) {
    var on = met(form, wrapper);
    var controls = wrapper.querySelectorAll('input:not([type="hidden"]), select, textarea, button');
    wrapper.getAttribute('data-lat-trigger-action').split('|').forEach(function (action) {
      if (action === 'show') conceal(wrapper, controls, !on);
      else if (action === 'hide') conceal(wrapper, controls, on);
      else if (action === 'enable') controls.forEach(function (c) { c.disabled = !on; });
      else if (action === 'disable') controls.forEach(function (c) { c.disabled = on; });
      else if (on && action === 'empty') controls.forEach(function (c) {
        if (c.type === 'checkbox' || c.type === 'radio') c.checked = false;
        else if (c.tagName !== 'BUTTON') c.value = '';
      });
      else if (on && action.indexOf('fill[') === 0) {
        var value = action.slice(5, -1);
        controls.forEach(function (c) {
          if (c.type === 'checkbox' || c.type === 'radio') c.checked = c.value === value;
          else if (c.tagName !== 'BUTTON') c.value = value;
        });
      }
    });
    if (wrapper._latMet !== on) {
      wrapper._latMet = on;
      window.lat.emit(wrapper, 'trigger:changed', { field: wrapper.getAttribute('data-lat-trigger-field'), met: on });
    }
  }
  function all(form) {
    form.querySelectorAll('[data-lat-trigger-field]').forEach(function (w) { apply(form, w); });
  }
  function watching(e) {
    var form = e.target.form || (e.target.closest && e.target.closest('form'));
    if (form && form.querySelector('[data-lat-trigger-field]')) all(form);
  }
  document.addEventListener('change', watching);
  document.addEventListener('input', watching);
  function start(root) {
    var scope = root && root.querySelectorAll ? root : document;
    var forms = Array.prototype.slice.call(scope.querySelectorAll('form'));
    // A swapped-in fragment is often the form itself.
    if (scope.matches && scope.matches('form')) forms.push(scope);
    forms.forEach(function (form) {
      if (form.querySelector('[data-lat-trigger-field]')) all(form);
    });
  }
  document.addEventListener('DOMContentLoaded', function () { start(document); });
  document.addEventListener('htmx:load', function (e) { start(e.target); });
})();

// Rows: a row that names its record (`data-lat-href`) opens it on a click or
// Enter, as its Edit link would. A click on a control in the row is that
// control's, and text being selected is left to be copied. A modified click
// or a middle click opens the record in a new tab. `row_click: none` on the
// list renders rows without the attribute.
(function () {
  function target(e) {
    var row = e.target.closest ? e.target.closest('tr[data-lat-href]') : null;
    if (!row) return null;
    if (e.target.closest('a, button, input, select, textarea, label, form, [data-lat-widget]')) return null;
    return row;
  }
  function open(url, newTab) {
    if (newTab) window.open(url, '_blank', 'noopener');
    else window.location.href = url;
  }
  document.addEventListener('click', function (e) {
    if (e.defaultPrevented || e.button !== 0) return;
    var row = target(e);
    if (!row) return;
    var selection = window.getSelection ? window.getSelection() : null;
    if (selection && String(selection) !== '' && row.contains(selection.anchorNode)) return;
    open(row.getAttribute('data-lat-href'), e.metaKey || e.ctrlKey);
  });
  document.addEventListener('auxclick', function (e) {
    if (e.button !== 1) return;
    var row = target(e);
    if (!row) return;
    e.preventDefault();
    open(row.getAttribute('data-lat-href'), true);
  });
  document.addEventListener('keydown', function (e) {
    var row = e.target;
    if (e.key !== 'Enter' || !row.matches || !row.matches('tr[data-lat-href]')) return;
    e.preventDefault();
    open(row.getAttribute('data-lat-href'), e.metaKey || e.ctrlKey);
  });
})();

// Reorder: a list in its own order carries a grip on every row. Dragging a
// row drops it before or after another and posts the move; the arrows beside
// the grip are the keyboard path and post the same way. The island sits on
// the header cell, outside the rows, so a row click stays the row's. After
// the region swaps back in, focus returns to the arrow that was pressed.
(function () {
  var pending = null;
  window.lat.widget('reorder', function (th) {
    var table = th.closest('table');
    var path = th.getAttribute('data-reorder-path');
    var region = th.getAttribute('data-reorder-region');
    if (!table || !path || !window.htmx) return;
    var dragging = null;
    function rows() { return Array.prototype.slice.call(table.querySelectorAll('tbody tr[data-lat-id]')); }
    function clear() {
      rows().forEach(function (r) { r.classList.remove('is-drop-before', 'is-drop-after'); });
    }
    // Under a tree, a child's siblings come back as rows: the old ones go
    // and the response lands after the parent, which stays expanded.
    function siblings(row) {
      var parent = row.getAttribute('data-lat-parent');
      if (!parent) return null;
      var parentRow = table.querySelector('tr[data-lat-id="' + parent + '"]');
      if (!parentRow) return null;
      var depth = +parentRow.getAttribute('data-lat-depth') || 0;
      var next = parentRow.nextElementSibling;
      while (next && (+next.getAttribute('data-lat-depth') || 0) > depth) {
        var gone = next;
        next = next.nextElementSibling;
        gone.remove();
      }
      return parentRow;
    }
    function post(values, row) {
      values._csrf = th.getAttribute('data-reorder-csrf');
      values.page = th.getAttribute('data-reorder-page');
      values.parent = row.getAttribute('data-lat-parent') || '';
      values.depth = row.getAttribute('data-lat-depth') || '';
      var parentRow = siblings(row);
      if (parentRow) {
        window.htmx.ajax('POST', path, { target: parentRow, swap: 'afterend', values: values });
      } else {
        window.htmx.ajax('POST', path, { target: '#' + region, swap: 'outerHTML', values: values });
      }
    }
    table.addEventListener('dragstart', function (e) {
      var grip = e.target.closest ? e.target.closest('.lat-grip') : null;
      var row = grip && grip.closest('tr[data-lat-id]');
      if (!row) return;
      dragging = row;
      row.classList.add('is-dragging');
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', row.getAttribute('data-lat-id'));
    });
    table.addEventListener('dragover', function (e) {
      if (!dragging) return;
      var row = e.target.closest ? e.target.closest('tr[data-lat-id]') : null;
      if (!row || row === dragging) return;
      e.preventDefault();
      var box = row.getBoundingClientRect();
      var before = e.clientY < box.top + box.height / 2;
      clear();
      row.classList.add(before ? 'is-drop-before' : 'is-drop-after');
    });
    table.addEventListener('drop', function (e) {
      if (!dragging) return;
      var row = e.target.closest ? e.target.closest('tr[data-lat-id]') : null;
      e.preventDefault();
      if (row && row !== dragging && row.getAttribute('data-lat-parent') === dragging.getAttribute('data-lat-parent')) {
        var before = row.classList.contains('is-drop-before');
        var values = { id: dragging.getAttribute('data-lat-id') };
        values[before ? 'before' : 'after'] = row.getAttribute('data-lat-id');
        post(values, dragging);
      }
      clear();
    });
    table.addEventListener('dragend', function () {
      if (dragging) dragging.classList.remove('is-dragging');
      dragging = null;
      clear();
    });
    // Remember which arrow was pressed, to focus it again after the swap. An
    // arrow under a tree parent posts through the island, so the response
    // replaces the siblings rather than the whole region.
    table.addEventListener('submit', function (e) {
      var form = e.target;
      if (!form.classList.contains('lat-grip__moves')) return;
      var button = form.querySelector('.lat-grip__move:focus') || e.submitter;
      var row = form.closest('tr[data-lat-id]');
      if (button && row) pending = { id: row.getAttribute('data-lat-id'), move: button.value };
      if (row && row.getAttribute('data-lat-parent') && button) {
        e.preventDefault();
        post({ id: row.getAttribute('data-lat-id'), move: button.value }, row);
      }
    });
    if (pending) {
      var row = table.querySelector('tr[data-lat-id="' + pending.id + '"]');
      var button = row && row.querySelector('.lat-grip__move[value="' + pending.move + '"]');
      pending = null;
      if (button) (button.disabled ? button.form.querySelector('.lat-grip__move:not(:disabled)') : button).focus();
    }
  });
})();

// Tree: a row with children carries a toggle that loads them in place; the
// same toggle takes them out again. The toggle is a link, so with scripting
// off it opens the children as a page of their own. The island sits on the
// first header cell, outside the rows, so a row click stays the row's.
(function () {
  window.lat.widget('tree', function (th) {
    var table = th.closest('table');
    if (!table || !window.htmx) return;
    table.addEventListener('click', function (e) {
      var toggle = e.target.closest ? e.target.closest('.lat-tree__toggle') : null;
      if (!toggle || !table.contains(toggle)) return;
      e.preventDefault();
      var row = toggle.closest('tr[data-lat-id]');
      if (!row) return;
      var depth = +row.getAttribute('data-lat-depth') || 0;
      if (toggle.getAttribute('aria-expanded') === 'true') {
        var next = row.nextElementSibling;
        while (next && (+next.getAttribute('data-lat-depth') || 0) > depth) {
          var gone = next;
          next = next.nextElementSibling;
          gone.remove();
        }
        toggle.setAttribute('aria-expanded', 'false');
        toggle.classList.remove('is-expanded');
        return;
      }
      toggle.setAttribute('aria-expanded', 'true');
      toggle.classList.add('is-expanded');
      window.htmx.ajax('GET', toggle.getAttribute('href'), { source: toggle, target: row, swap: 'afterend' });
    });
  });
})();

// Editable cell: the form saves itself on change, so the Save button is for
// a page without scripting. Starting the island marks the form ready, and the
// stylesheet hides the button on a ready cell; the button's own display rule
// would beat the hidden attribute.
window.lat.widget('cell', function () {});

// Menu order: the top bar's tabs drag into the order the operator wants, and
// the order is kept with their preferences, so it follows them. Dashboard and
// Settings stay put; the others move among themselves.
window.lat.widget('menu-order', function (nav) {
  var post = nav.getAttribute('data-menu-post');
  if (!post) return;
  var dragging = null;
  function tabs() { return Array.prototype.slice.call(nav.querySelectorAll('[data-lat-key]')); }
  function clear() { tabs().forEach(function (t) { t.classList.remove('is-drop-before', 'is-drop-after'); }); }
  nav.addEventListener('dragstart', function (e) {
    var tab = e.target.closest ? e.target.closest('[draggable="true"]') : null;
    if (!tab) return;
    dragging = tab;
    tab.classList.add('is-dragging');
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/plain', tab.getAttribute('data-lat-key'));
  });
  nav.addEventListener('dragover', function (e) {
    if (!dragging) return;
    var tab = e.target.closest ? e.target.closest('[draggable="true"]') : null;
    if (!tab || tab === dragging) return;
    e.preventDefault();
    var box = tab.getBoundingClientRect();
    clear();
    tab.classList.add(e.clientX < box.left + box.width / 2 ? 'is-drop-before' : 'is-drop-after');
  });
  nav.addEventListener('drop', function (e) {
    if (!dragging) return;
    var tab = e.target.closest ? e.target.closest('[draggable="true"]') : null;
    e.preventDefault();
    if (tab && tab !== dragging) {
      tab.insertAdjacentElement(tab.classList.contains('is-drop-before') ? 'beforebegin' : 'afterend', dragging);
      var order = tabs().map(function (t) { return t.getAttribute('data-lat-key'); }).join(',');
      var body = new URLSearchParams();
      body.set('_csrf', nav.getAttribute('data-menu-csrf') || '');
      body.set('order', order);
      fetch(post, { method: 'POST', body: body, credentials: 'same-origin', headers: { 'X-Requested-With': 'fetch' } })
        .catch(function () { window.lat.flash && window.lat.flash('The menu order could not be saved.', 'error'); });
    }
    clear();
  });
  nav.addEventListener('dragend', function () {
    if (dragging) dragging.classList.remove('is-dragging');
    dragging = null;
    clear();
  });
});

// Command palette: one box that finds a page, a setting or a record and goes
// there. The search button opens it, as does Ctrl+K (Cmd+K on a Mac); the
// arrows move, Enter goes, Escape closes. Without scripting the button is a
// link to the same finder as a page.
window.lat.widget('palette', function (button) {
  var url = button.getAttribute('data-palette-url');
  if (!url) return;
  var overlay = document.createElement('div');
  overlay.className = 'lat-palette';
  overlay.setAttribute('role', 'dialog');
  overlay.setAttribute('aria-modal', 'true');
  overlay.innerHTML =
    '<div class="lat-palette__backdrop" data-palette-close></div>' +
    '<div class="lat-palette__panel">' +
    '<input class="lat-palette__input" type="text" autocomplete="off" role="combobox" aria-expanded="true" aria-autocomplete="list">' +
    '<ul class="lat-palette__list" role="listbox"></ul>' +
    '</div>';
  document.body.appendChild(overlay);
  var input = overlay.querySelector('input');
  var list = overlay.querySelector('ul');
  input.placeholder = button.getAttribute('data-palette-placeholder') || '';
  var active = -1;
  var timer = null;
  var latest = 0;
  function items() { return Array.prototype.slice.call(list.querySelectorAll('.lat-palette__hit')); }
  function mark(index) {
    var all = items();
    if (!all.length) { active = -1; return; }
    active = (index + all.length) % all.length;
    all.forEach(function (a, i) { a.classList.toggle('is-active', i === active); });
    all[active].scrollIntoView({ block: 'nearest' });
  }
  function show(found) {
    list.innerHTML = '';
    if (!found.length) {
      var empty = document.createElement('li');
      empty.className = 'lat-palette__empty';
      empty.textContent = button.getAttribute('data-palette-empty') || '';
      list.appendChild(empty);
      active = -1;
      return;
    }
    found.forEach(function (hit) {
      var li = document.createElement('li');
      var a = document.createElement('a');
      a.className = 'lat-palette__hit';
      a.href = hit.href;
      a.setAttribute('role', 'option');
      var label = document.createElement('span');
      label.className = 'lat-palette__label';
      label.textContent = hit.label;
      a.appendChild(label);
      if (hit.hint) {
        var hint = document.createElement('span');
        hint.className = 'lat-palette__hint';
        hint.textContent = hit.hint;
        a.appendChild(hint);
      }
      li.appendChild(a);
      list.appendChild(li);
    });
    mark(0);
  }
  function search() {
    var seq = ++latest;
    fetch(url + '?q=' + encodeURIComponent(input.value), { credentials: 'same-origin', headers: { 'X-Requested-With': 'fetch' } })
      .then(function (r) { return r.json(); })
      .then(function (data) { if (seq === latest) show(data.items || []); })
      .catch(function () { if (seq === latest) show([]); });
  }
  function open() {
    overlay.classList.add('is-open');
    input.value = '';
    input.focus();
    search();
  }
  function close() {
    overlay.classList.remove('is-open');
    button.focus();
  }
  button.addEventListener('click', function (e) { e.preventDefault(); open(); });
  overlay.addEventListener('click', function (e) {
    if (e.target.closest('[data-palette-close]')) close();
  });
  input.addEventListener('input', function () {
    clearTimeout(timer);
    timer = setTimeout(search, 150);
  });
  input.addEventListener('keydown', function (e) {
    if (e.key === 'ArrowDown') { e.preventDefault(); mark(active + 1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); mark(active - 1); }
    else if (e.key === 'Enter') {
      var all = items();
      if (active >= 0 && all[active]) { e.preventDefault(); window.location.href = all[active].href; }
    } else if (e.key === 'Escape') { e.preventDefault(); close(); }
  });
  document.addEventListener('keydown', function (e) {
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && (e.key === 'k' || e.key === 'K')) {
      e.preventDefault();
      if (overlay.classList.contains('is-open')) close(); else open();
    }
  });
});
